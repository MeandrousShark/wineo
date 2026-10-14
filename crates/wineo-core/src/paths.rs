use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::winpath;

/// Selects a portable folder when `--portable` is not given.
pub const PORTABLE_VAR: &str = "WINEO_PORTABLE_DIR";

/// Where wineo keeps its files, following the XDG base directory spec unless a
/// portable folder is selected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paths {
    home: PathBuf,
    config: PathBuf,
    data: PathBuf,
    state: PathBuf,
    /// The XDG state dir, shared by every root, for locks that have to be.
    host_state: PathBuf,
    /// The XDG cache dir, which belongs to the host even in a portable folder.
    cache: PathBuf,
    portable: Option<PathBuf>,
}

impl Paths {
    /// Resolves paths from the process environment. `portable` is the
    /// `--portable` option, which wins over `WINEO_PORTABLE_DIR`.
    pub fn from_env(portable: Option<&Path>) -> Result<Self> {
        Self::select(|key| std::env::var_os(key), portable)
    }

    /// Resolves paths from an arbitrary variable source, so tests don't have
    /// to mutate the process environment.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<OsString>) -> Result<Self> {
        Self::select(lookup, None)
    }

    pub fn select(
        lookup: impl Fn(&str) -> Option<OsString>,
        portable: Option<&Path>,
    ) -> Result<Self> {
        let home = lookup("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or(Error::NoHome)?;
        // The spec says relative values are invalid and must be ignored.
        let base = |var: &str, fallback: &str| {
            lookup(var)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
                .join("wineo")
        };
        let state = base("XDG_STATE_HOME", ".local/state");
        let mut paths = Self {
            config: base("XDG_CONFIG_HOME", ".config"),
            data: base("XDG_DATA_HOME", ".local/share"),
            cache: base("XDG_CACHE_HOME", ".cache"),
            host_state: state.clone(),
            state,
            home,
            portable: None,
        };
        let selected = match portable {
            Some(root) if root.as_os_str().is_empty() => return Err(Error::EmptyPortable),
            Some(root) => Some(root.to_owned()),
            None => lookup(PORTABLE_VAR)
                .filter(|root| !root.is_empty())
                .map(PathBuf::from),
        };
        if let Some(root) = selected {
            let root = match root.to_str() {
                Some(text) => paths.expand(text),
                None => root,
            };
            let root = resolve(
                &std::path::absolute(&root)
                    .context(|| format!("failed to resolve {}", root.display()))?,
            );
            // Dotted, so it never collides with the runtime dir of an XDG
            // environment, and keyed by root so two folders never share state.
            let key = format!(
                "{}-{:016x}",
                root.file_name()
                    .and_then(|name| name.to_str())
                    .map(label)
                    .unwrap_or_default(),
                fnv1a(root.as_os_str().as_bytes())
            );
            paths.state = paths.state.join(".portable").join(key);
            paths.config = root.join("config");
            paths.portable = Some(root);
        }
        Ok(paths)
    }

    /// The selected portable folder, if any.
    pub fn portable_root(&self) -> Option<&Path> {
        self.portable.as_deref()
    }

    pub fn global_config(&self) -> PathBuf {
        self.config.join("config.toml")
    }

    pub fn environments_dir(&self) -> PathBuf {
        self.config.join("environments")
    }

    /// A template is configuration, not data, so it sits beside the configs
    /// rather than beside the builds it names.
    pub fn templates_dir(&self) -> PathBuf {
        self.config.join("templates")
    }

    pub fn environment_config(&self, name: &str) -> PathBuf {
        self.environments_dir().join(format!("{name}.toml"))
    }

    /// Durable trash metadata, carried by a portable folder when selected.
    pub fn trash_dir(&self) -> PathBuf {
        self.portable
            .as_ref()
            .map_or_else(|| self.data.join("trash"), |root| root.join("data/trash"))
    }

    pub fn snapshot_root(&self, prefix: &Path, name: &str) -> PathBuf {
        prefix
            .parent()
            .expect("a prefix root has a parent")
            .join(SNAPSHOTS)
            .join(name)
    }

    /// The XDG applications directory. Desktop entries have to sit in the
    /// standard location, not under wineo's own data dir, or nothing reads them.
    pub fn applications_dir(&self) -> PathBuf {
        self.xdg_data().join("applications")
    }

    /// Where wine is pointed while an icon is extracted, so the icon theme it
    /// insists on writing never lands in the real one.
    pub fn icon_scratch_dir(&self) -> PathBuf {
        self.state.join(".icons")
    }

    /// The XDG icon theme root, for the same reason.
    pub fn icons_dir(&self) -> PathBuf {
        self.xdg_data().join("icons")
    }

    pub(crate) fn home(&self) -> &Path {
        &self.home
    }

    /// `$XDG_CONFIG_HOME` itself. None in a portable folder, whose config dir
    /// is its own rather than the host's.
    pub(crate) fn xdg_config(&self) -> Option<&Path> {
        if self.portable.is_some() {
            return None;
        }
        self.config.parent()
    }

    /// `$XDG_DATA_HOME` itself, which every wineo data path hangs off.
    pub(crate) fn xdg_data(&self) -> &Path {
        self.data
            .parent()
            .expect("the data dir is always `<xdg data home>/wineo`")
    }

    pub fn default_prefix_root(&self) -> PathBuf {
        match &self.portable {
            Some(root) => root.join("prefixes"),
            None => self.data.join("prefixes"),
        }
    }

    /// Where GOG games go when `defaults.games_root` is unset. Always under
    /// the home directory, since GOG games live in the host library.
    pub fn default_games_root(&self) -> PathBuf {
        self.home.join("Games")
    }

    /// The XDG data dir, even in a portable folder, for what belongs to this
    /// machine rather than the folder, like the SteamGridDB key.
    pub fn host_data(&self) -> &Path {
        &self.data
    }

    /// An environment's artwork. A portable folder carries it, so it moves
    /// with the folder.
    pub fn art_dir(&self, name: &str) -> PathBuf {
        match &self.portable {
            Some(root) => root.join("art").join(name),
            None => self.data.join("art").join(name),
        }
    }

    /// Thumbnails of stored art. Always on the host, since a cache is cheap to
    /// rebuild and a portable folder shouldn't carry it.
    pub fn art_cache_dir(&self) -> PathBuf {
        self.cache.join("art")
    }

    /// GOG's box art for games a user owns but wineo has no art for yet, one
    /// `<id>.<ext>` each. Beside `art_cache_dir`, not inside it, since that one
    /// holds thumbnails of stored art. On the host like any cache.
    pub fn gog_covers_dir(&self) -> PathBuf {
        self.cache.join("gog-covers")
    }

    /// The cached list of Steam games an account owns. Always on the host,
    /// like any cache.
    pub fn steam_owned_cache(&self, steam_id: u64) -> PathBuf {
        self.cache.join(format!("steam-owned-{steam_id}.json"))
    }

    /// The cached list of GOG games an account owns, on the host like
    /// Steam's.
    pub fn gog_owned_cache(&self, user_id: u64) -> PathBuf {
        self.cache.join(format!("gog-owned-{user_id}.json"))
    }

    /// The lock that GOG login, logout, and renewal share. Host-wide, since
    /// the saved login is too.
    pub fn gog_credentials_lock(&self) -> PathBuf {
        self.host_state.join(".locks").join("gog-credentials.lock")
    }

    /// A counter bumped whenever the saved GOG login changes, so a session
    /// can tell cheaply that it has to look again.
    pub fn gog_credentials_generation(&self) -> PathBuf {
        self.host_state
            .join(".locks")
            .join("gog-credentials.generation")
    }

    /// A GOG product's cached details. They are the same for every account.
    pub fn gog_product_cache(&self, product_id: u64) -> PathBuf {
        self.cache.join(format!("gog-product-{product_id}.json"))
    }

    /// GOG installer downloads, as `<product id>/<download id>/<file name>`.
    /// On the host, like any cache, and kept while a journal names them.
    pub fn gog_download_cache(&self) -> PathBuf {
        self.cache.join("gog-downloads")
    }

    /// GOG game folder state files. Always in the host data dir, since GOG
    /// games live in the host library only.
    pub fn gog_folders_dir(&self) -> PathBuf {
        self.data.join("gog").join("folders")
    }

    /// A GOG game folder's lock, keyed like its state file. Host-wide, so
    /// every root sees the same lock.
    pub fn gog_folder_lock(&self, key: &str) -> PathBuf {
        self.host_state
            .join(".locks")
            .join(format!("gog-folder-{key}.lock"))
    }

    /// Held while a migration adopts a GOG game folder, so two adoptions
    /// can't both pass the overlap check. Host-wide, like the folders.
    pub fn gog_adoption_lock(&self) -> PathBuf {
        self.host_state.join(".locks").join("gog-adoption.lock")
    }

    /// A GOG game folder's in-use lock, which launches hold for their whole
    /// run. Host-wide like the state lock.
    pub fn gog_folder_in_use_lock(&self, key: &str) -> PathBuf {
        self.host_state
            .join(".locks")
            .join(format!("gog-folder-{key}.in-use.lock"))
    }

    /// The records naming what holds a GOG game folder's in-use lock: one
    /// file per holder. Host-wide like the lock.
    pub fn gog_folder_holders(&self, key: &str) -> PathBuf {
        self.host_state
            .join(".locks")
            .join(format!("gog-folder-{key}.holders"))
    }

    /// An environment's playtime log. A portable folder carries it, so it
    /// moves with the folder, like art.
    pub fn stats_log(&self, name: &str) -> PathBuf {
        match &self.portable {
            Some(root) => root.join("stats").join(format!("{name}.jsonl")),
            None => self.data.join("stats").join(format!("{name}.jsonl")),
        }
    }

    pub fn default_runner_root(&self) -> PathBuf {
        self.data.join("runners")
    }

    pub fn default_dxvk_root(&self) -> PathBuf {
        self.data.join("dxvk")
    }

    pub fn default_vkd3d_root(&self) -> PathBuf {
        self.data.join("vkd3d")
    }

    /// Operation lock files. Dotted, so it can never collide with the runtime
    /// dir of an environment, and stable, so removing one leaves it alone.
    pub fn locks_dir(&self) -> PathBuf {
        self.state.join(".locks")
    }

    /// Prefix lock files. Host-wide rather than per root, so environments in
    /// different roots that touch the same prefix still coordinate.
    pub fn prefix_locks_dir(&self) -> PathBuf {
        self.host_state.join(".locks")
    }

    /// Builds can be selected from different roots, so their locks stay on the host.
    pub fn build_locks_dir(&self) -> PathBuf {
        self.host_state.join(".locks")
    }

    /// Where an interrupted snapshot restore of `name` leaves its marker. A
    /// portable one sits beside the snapshots, so it moves with the folder.
    pub fn restore_marker(&self, name: &str) -> PathBuf {
        restore_marker(&self.locks_dir(), self.portable.as_deref(), name)
    }

    pub fn runtime_dir(&self, name: &str) -> PathBuf {
        self.state.join(name)
    }

    /// Where a Heroic import appends its `-v` log, one file per environment name.
    pub fn import_log(&self, name: &str) -> PathBuf {
        self.state.join("imports").join(format!("{name}.log"))
    }

    /// Expands a leading `~` in a path taken from a config file or the command line.
    pub fn expand(&self, path: &str) -> PathBuf {
        if path == "~" {
            return self.home.clone();
        }
        match path.strip_prefix("~/") {
            Some(rest) => self.home.join(rest),
            None => PathBuf::from(path),
        }
    }

    /// Expands `~` and resolves relative paths against the current directory.
    pub fn absolute(&self, path: &str) -> Result<PathBuf> {
        std::path::absolute(self.expand(path)).context(|| format!("failed to resolve {path}"))
    }

    /// Turns a path back into its config form, writing paths under the home
    /// directory as `~/...` so configs survive a move to another machine.
    pub fn contract(&self, path: &Path) -> Result<String> {
        let text = |path: &Path| {
            path.to_str()
                .map(str::to_owned)
                .ok_or_else(|| Error::NonUtf8Path(path.to_owned()))
        };
        match path.strip_prefix(&self.home) {
            Ok(rest) if rest.as_os_str().is_empty() => Ok("~".to_owned()),
            Ok(rest) => Ok(format!("~/{}", text(rest)?)),
            Err(_) => text(path),
        }
    }

    /// `contract` for every path under the home directory inside a message,
    /// like the paths an error names. Only a whole path is contracted, so
    /// `/home/ann2` or `/mnt/home/ann` is left alone when home is `/home/ann`.
    pub fn contract_text(&self, text: &str) -> String {
        let Some(home) = self.home.to_str().filter(|home| *home != "/") else {
            return text.to_owned();
        };
        let in_path = |char: char| char.is_alphanumeric() || matches!(char, '/' | '-' | '_' | '.');
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find(home) {
            let before = match at {
                0 => out.chars().last(),
                _ => rest[..at].chars().last(),
            };
            let mut after = rest[at + home.len()..].chars();
            // A period ending a sentence isn't part of the path.
            let ends = match after.next() {
                None | Some('/') => true,
                Some('.') => !after.next().is_some_and(in_path),
                Some(char) => !in_path(char),
            };
            out.push_str(&rest[..at]);
            let starts = !before.is_some_and(in_path);
            out.push_str(if starts && ends { "~" } else { home });
            rest = &rest[at + home.len()..];
        }
        out.push_str(rest);
        out
    }

    /// The config form of a path. Inside a portable folder it is relative to
    /// the root, so the folder can move. Elsewhere it is `contract`.
    pub fn store(&self, path: &Path) -> Result<String> {
        if let Some(root) = &self.portable {
            let resolved = resolve(path);
            if let Ok(rest) = resolved.strip_prefix(root) {
                if rest.as_os_str().is_empty() {
                    return Ok(".".to_owned());
                }
                return rest
                    .to_str()
                    .map(str::to_owned)
                    .ok_or_else(|| Error::NonUtf8Path(resolved.clone()));
            }
        }
        self.contract(path)
    }

    /// Resolves a Unix path read from a config file. A portable folder
    /// resolves relative paths from its root, independent of the current
    /// directory. Elsewhere this is `absolute`.
    pub fn config_path(&self, value: &str) -> Result<PathBuf> {
        match &self.portable {
            Some(root) => {
                let path = self.expand(value);
                Ok(if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                })
            }
            None => self.absolute(value),
        }
    }

    /// How a stored path is shown. A portable one is relative to a root the
    /// reader can't see, so it is shown resolved. Elsewhere it is kept as written.
    pub fn shown(&self, value: &str) -> String {
        match &self.portable {
            Some(_) => self
                .config_path(value)
                .map_or_else(|_| value.to_owned(), |path| path.display().to_string()),
            None => value.to_owned(),
        }
    }

    /// Like `config_path`, but Windows paths are left to wine, so they give None.
    pub fn resolve_stored(&self, value: &str) -> Result<Option<PathBuf>> {
        if winpath::is_windows_path(value) {
            return Ok(None);
        }
        self.config_path(value).map(Some)
    }

    /// The config form of a target path. In a portable folder, a Unix path
    /// inside it becomes relative to the root. Anything else, and every value
    /// outside portable mode, is kept as written.
    pub fn store_value(&self, value: &str) -> Result<String> {
        if self.portable.is_none() || winpath::is_windows_path(value) {
            return Ok(value.to_owned());
        }
        let path = self.expand(value);
        if path.is_absolute() && self.portable_relative(&path).is_some() {
            return self.store(&path);
        }
        Ok(value.to_owned())
    }

    /// `path` relative to the portable root, when a folder is selected and the
    /// path is inside it.
    pub fn portable_relative(&self, path: &Path) -> Option<PathBuf> {
        let root = self.portable.as_ref()?;
        resolve(path).strip_prefix(root).ok().map(Path::to_owned)
    }

    /// Refuses a prefix a portable folder can't carry. Always passes outside
    /// portable mode.
    pub fn check_prefix(&self, prefix: &Path) -> Result<()> {
        let Some(root) = &self.portable else {
            return Ok(());
        };
        let prefixes = root.join("prefixes");
        // Snapshots and restore markers are found beside the prefix, which
        // only lines up with the folder's layout for a direct child.
        let child = resolve(prefix)
            .strip_prefix(&prefixes)
            .ok()
            .is_some_and(|rest| {
                let mut parts = rest.components();
                matches!(parts.next(), Some(Component::Normal(name)) if name != SNAPSHOTS)
                    && parts.next().is_none()
            });
        if child {
            Ok(())
        } else {
            Err(Error::PrefixOutsidePortable(prefixes))
        }
    }

    /// Refuses a prefix root other than the portable folder's own.
    pub fn check_prefix_root(&self, dir: &Path) -> Result<()> {
        match &self.portable {
            Some(root) if resolve(dir) != root.join("prefixes") => {
                Err(Error::PortablePrefixRoot(root.join("prefixes")))
            }
            _ => Ok(()),
        }
    }

    /// The config form of a prefix, refusing one outside a portable folder.
    pub fn store_prefix(&self, prefix: &Path) -> Result<String> {
        self.check_prefix(prefix)?;
        self.store(prefix)
    }
}

/// The directory beside prefixes that holds their snapshots.
pub(crate) const SNAPSHOTS: &str = ".wineo-snapshots";

pub(crate) fn restore_marker(locks_dir: &Path, portable: Option<&Path>, name: &str) -> PathBuf {
    match portable {
        Some(root) => root
            .join("prefixes")
            .join(SNAPSHOTS)
            .join(name)
            .join("restore.json"),
        None => locks_dir.join(format!("snapshot-restore-{name}-restore.json")),
    }
}

pub(crate) fn label(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect()
}

/// FNV-1a, so the same path always maps to the same key, in any build.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Canonicalizes as much of `path` as exists, so two spellings of the same
/// directory compare equal even before it is created.
pub fn resolve(path: &Path) -> PathBuf {
    let components: Vec<Component> = path.components().collect();
    for split in (0..=components.len()).rev() {
        let head: PathBuf = components[..split].iter().collect();
        let Ok(base) = std::fs::canonicalize(&head) else {
            continue;
        };
        // Nothing past the longest existing ancestor exists, so none of it can
        // be a symlink and `..` is safe to fold in without touching the disk.
        let mut resolved = base;
        for component in &components[split..] {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    resolved.pop();
                }
                other => resolved.push(other),
            }
        }
        return resolved;
    }
    path.to_owned()
}

#[cfg(test)]
mod tests;
