//! Launch protection: which GOG folders a launch runs from, and whether it
//! may. Every launch from a folder holds its in-use lock until the run ends.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};

use super::{FolderError, FolderLock, FolderState, InUseGuard, InUseLock, find, key, read_file};
use crate::paths::Paths;

/// How many times a launch looks for its folders again when one appeared
/// while it was locking, before giving up.
const ATTEMPTS: usize = 3;

/// The GOG folders a launch touches, as found without locks.
struct Touched<'a> {
    paths: Paths,
    name: &'a str,
    gog_id: Option<&'a str>,
    /// By key, which is lock order.
    states: BTreeMap<String, FolderState>,
    /// The environment's own game folder.
    own: Option<PathBuf>,
    /// Whether that folder is ownerless and gets claimed.
    claim: bool,
}

impl<'a> Touched<'a> {
    fn find(
        paths: &Paths,
        name: &'a str,
        gog_id: Option<&'a str>,
        touched: &[PathBuf],
    ) -> Result<Self> {
        let mut matched = Vec::new();
        for path in touched.iter().filter(|path| path.is_absolute()) {
            if let Some(state) = folder_holding(paths, path)? {
                matched.push(state);
            }
        }

        // A portable root holds no GOG games, so a `gog_id` there names
        // nothing it could own, and only the paths it touches count.
        let gog_id = gog_id.filter(|_| paths.portable_root().is_none());
        let (own, claim) = match gog_id {
            None => (None, false),
            Some(gog_id) => match own_folder(paths, name, gog_id, &matched)? {
                Some((state, claim)) => (Some(state), claim),
                None => {
                    return Err(FolderError::StateMissing {
                        name: name.to_owned(),
                        gog_id: gog_id.to_owned(),
                    }
                    .into());
                }
            },
        };
        let own_folder = own.as_ref().map(|state| state.folder.clone());
        let mut states = BTreeMap::new();
        for state in own.into_iter().chain(matched) {
            states.entry(key(&state.folder)).or_insert(state);
        }
        Ok(Self {
            paths: paths.clone(),
            name,
            gog_id,
            states,
            own: own_folder,
            claim,
        })
    }

    /// Takes every in-use lock shared as this launch's environment, refusing
    /// one an update holds.
    fn lock_in_use(&self, paths: &Paths) -> Result<Vec<InUseGuard>> {
        let mut guards = Vec::new();
        for state in self.states.values() {
            match InUseLock::new(paths, &state.folder)?.try_shared(self.name)? {
                Some(guard) => guards.push(guard),
                None => {
                    return Err(FolderError::Updating {
                        name: state.label(),
                    }
                    .into());
                }
            }
        }
        Ok(guards)
    }

    /// Refuses a folder whose state is lost, taken, or mid-update. A folder
    /// to be claimed is judged as its claimant's.
    fn check(&self, folder: &Path, state: Option<FolderState>) -> Result<()> {
        let own = self.own.as_deref() == Some(folder);
        let Some(mut state) = state else {
            if own {
                return Err(FolderError::StateMissing {
                    name: self.name.to_owned(),
                    gog_id: self.gog_id.unwrap_or_default().to_owned(),
                }
                .into());
            }
            // Forgotten since it was found, so no longer a game folder.
            return Ok(());
        };
        if own && self.claim && state.owner.is_none() {
            state.owner = Some(self.name.to_owned());
        }
        if own
            && let Some(owner) = &state.owner
            && owner != self.name
        {
            return Err(FolderError::OwnedByOther {
                folder: folder.to_owned(),
                owner: owner.clone(),
            }
            .into());
        }
        if state.journal.is_some() {
            return Err(super::unfinished(&self.paths, &state).into());
        }
        Ok(())
    }
}

/// Checks a launch of `name` and takes the in-use lock of every GOG folder it
/// touches, in key order: its own game's when `gog_id` is set, and any folder
/// holding one of `touched`, the host paths it names. An ownerless folder of
/// its own game is claimed on the way. Refuses a folder that is mid-update
/// or being updated, and a game whose state is lost.
pub fn protect(
    paths: &Paths,
    name: &str,
    gog_id: Option<&str>,
    touched: &[PathBuf],
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<InUseGuard>> {
    let (found, guards) = lock_touched(paths, name, gog_id, touched)?;
    if found.claim
        && let Some(folder) = &found.own
    {
        FolderLock::exclusive(paths, folder, cancel)?.reclaim(name)?;
    }
    for folder in found.states.values().map(|state| &state.folder) {
        let state = {
            let lock = FolderLock::shared(paths, folder, cancel)?;
            lock.read().map_err(|error| FolderError::UnreadableFolder {
                folder: folder.clone(),
                detail: format!("{error:#}"),
            })?
        };
        found.check(folder, state)?;
    }
    Ok(guards)
}

/// An environment's game folder: the one whose state names it as owner and
/// has its `gog_id`, else an ownerless one of its game among `matched`, the
/// states of the folders it names, which it may claim (the bool). None when
/// there is neither, a lost state. More than one ownerless candidate is
/// refused, and so is a folder of its game another environment owns.
fn own_folder(
    paths: &Paths,
    name: &str,
    gog_id: &str,
    matched: &[FolderState],
) -> Result<Option<(FolderState, bool)>> {
    if let Some(state) = find(paths, name, gog_id)? {
        return Ok(Some((state, false)));
    }
    let mut games: Vec<&FolderState> = Vec::new();
    for state in matched.iter().filter(|state| state.is_game(gog_id)) {
        if !games.iter().any(|seen| seen.folder == state.folder) {
            games.push(state);
        }
    }
    let ownerless: Vec<&FolderState> = games
        .iter()
        .copied()
        .filter(|state| state.owner.is_none())
        .collect();
    match ownerless.as_slice() {
        [one] => Ok(Some(((*one).clone(), true))),
        [] => match games.first() {
            Some(taken) => Err(FolderError::OwnedByOther {
                folder: taken.folder.clone(),
                owner: taken.owner.clone().unwrap_or_default(),
            }
            .into()),
            None => Ok(None),
        },
        many => Err(FolderError::SeveralFolders {
            name: name.to_owned(),
            folders: many.iter().map(|state| state.folder.clone()).collect(),
        }
        .into()),
    }
}

/// `name`'s game folder, found the way a launch finds it from `touched`,
/// the host paths its targets name, without claiming or locking anything.
/// An ownerless folder of its game comes back with no owner, for the
/// caller to claim under the state lock or only read.
pub fn game_folder(
    paths: &Paths,
    name: &str,
    gog_id: &str,
    touched: &[PathBuf],
) -> Result<Option<FolderState>> {
    // Its own folder first, so a broken state it owns is reported as its.
    if let Some(state) = find(paths, name, gog_id)? {
        return Ok(Some(state));
    }
    let mut matched = Vec::new();
    for path in touched.iter().filter(|path| path.is_absolute()) {
        if let Some(state) = folder_holding(paths, path)? {
            matched.push(state);
        }
    }
    Ok(own_folder(paths, name, gog_id, &matched)?.map(|(state, _)| state))
}

/// Finds the folders a launch touches and takes their in-use locks. They're
/// found without locks, so they're looked for again once locked: a folder
/// that became a game folder meanwhile means starting over, which keeps the
/// locks in key order.
fn lock_touched<'a>(
    paths: &Paths,
    name: &'a str,
    gog_id: Option<&'a str>,
    touched: &[PathBuf],
) -> Result<(Touched<'a>, Vec<InUseGuard>)> {
    for _ in 0..ATTEMPTS {
        let found = Touched::find(paths, name, gog_id, touched)?;
        between_find_and_lock();
        let guards = found.lock_in_use(paths)?;
        let again = Touched::find(paths, name, gog_id, touched)?;
        if again
            .states
            .keys()
            .all(|key| found.states.contains_key(key))
        {
            return Ok((again, guards));
        }
    }
    bail!("{name}'s GOG folders changed while locking; retry")
}

#[cfg(test)]
thread_local! {
    static BETWEEN: std::cell::RefCell<Option<Box<dyn FnMut() -> bool>>> =
        const { std::cell::RefCell::new(None) };
}

/// Runs a test's hook between finding folders and locking them. It's taken
/// out while it runs, so it can launch too, and stays armed while it
/// returns true.
fn between_find_and_lock() {
    #[cfg(test)]
    if let Some(mut hook) = BETWEEN.with(|hook| hook.borrow_mut().take())
        && hook()
    {
        BETWEEN.with(|slot| *slot.borrow_mut() = Some(hook));
    }
}

/// What `protect` would refuse, found without claiming, writing, or keeping
/// any lock, so a launch can be refused before its hooks run. `protect`
/// still decides once the launch holds its operation lock.
pub fn precheck(
    paths: &Paths,
    name: &str,
    gog_id: Option<&str>,
    touched: &[PathBuf],
) -> Result<()> {
    let found = Touched::find(paths, name, gog_id, touched)?;
    drop(found.lock_in_use(paths)?);
    for state in found.states.values() {
        found.check(&state.folder, Some(state.clone()))?;
    }
    Ok(())
}

/// The state of the GOG folder holding `path`, if any. Only the state files
/// of `path`'s own ancestors are looked at, so a broken state elsewhere
/// never matters, and nested game folders can't exist, so at most one
/// matches. One that exists but can't be read fails closed.
pub fn folder_holding(paths: &Paths, path: &Path) -> Result<Option<FolderState>> {
    let resolved = resolve(path);
    for folder in resolved.ancestors() {
        let file = super::state_path(paths, folder);
        let unreadable = |detail: String| FolderError::UnreadableFolder {
            folder: folder.to_owned(),
            detail,
        };
        match fs::symlink_metadata(&file) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(
                    unreadable(format!("failed to inspect {}: {error}", file.display())).into(),
                );
            }
        }
        match read_file(&file).map_err(|error| unreadable(format!("{error:#}")))? {
            None => continue,
            Some(state) if state.folder != folder => {
                return Err(unreadable(format!(
                    "{} names {}",
                    file.display(),
                    state.folder.display()
                ))
                .into());
            }
            Some(state) => return Ok(Some(state)),
        }
    }
    Ok(None)
}

/// `path` canonicalized, or for one that doesn't exist, its nearest existing
/// ancestor canonicalized with the rest appended.
fn resolve(path: &Path) -> PathBuf {
    let parts: Vec<Component> = path.components().collect();
    for split in (1..=parts.len()).rev() {
        let head: PathBuf = parts[..split].iter().collect();
        let Ok(mut resolved) = fs::canonicalize(&head) else {
            continue;
        };
        // Nothing below a missing directory exists, so no symlink can make
        // `..` there mean anything but the parent.
        for part in &parts[split..] {
            match part {
                Component::ParentDir => {
                    resolved.pop();
                }
                Component::Normal(name) => resolved.push(name),
                Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
            }
        }
        return resolved;
    }
    path.to_owned()
}

#[cfg(test)]
mod tests;
