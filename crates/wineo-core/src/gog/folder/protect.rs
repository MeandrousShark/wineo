//! Launch protection: which GOG folders a launch runs from, and whether it
//! may. Every launch from a folder holds its in-use lock until the run ends.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use anyhow::Result;

use super::{FolderError, FolderLock, FolderState, InUseGuard, InUseLock, find, key, read_file};
use crate::paths::Paths;

/// The GOG folders a launch touches, as found without locks.
struct Touched<'a> {
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
        let mut claim = false;
        let own = match gog_id {
            None => None,
            Some(gog_id) => match find(paths, name, gog_id)? {
                Some(state) => Some(state),
                None => {
                    let ownerless = matched
                        .iter()
                        .find(|state| state.owner.is_none() && state.is_game(gog_id))
                        .ok_or_else(|| FolderError::StateMissing {
                            name: name.to_owned(),
                            gog_id: gog_id.to_owned(),
                        })?;
                    claim = true;
                    Some(ownerless.clone())
                }
            },
        };
        let own_folder = own.as_ref().map(|state| state.folder.clone());
        let mut states = BTreeMap::new();
        for state in own.into_iter().chain(matched) {
            states.entry(key(&state.folder)).or_insert(state);
        }
        Ok(Self {
            name,
            gog_id,
            states,
            own: own_folder,
            claim,
        })
    }

    /// Takes every in-use lock shared, refusing one an update holds.
    fn lock_in_use(&self, paths: &Paths) -> Result<Vec<InUseGuard>> {
        let mut guards = Vec::new();
        for state in self.states.values() {
            match InUseLock::new(paths, &state.folder)?.try_shared()? {
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
        if let Some(journal) = &state.journal {
            return Err(match &state.owner {
                Some(owner) => FolderError::MidUpdate {
                    name: owner.clone(),
                },
                None => FolderError::OwnerlessMidUpdate {
                    gog_id: state.gog_id().unwrap_or(journal.target.gog_id),
                },
            }
            .into());
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
    let found = Touched::find(paths, name, gog_id, touched)?;
    let guards = found.lock_in_use(paths)?;
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
mod tests {
    use std::os::unix::fs::MetadataExt;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::super::{
        Committed, Journal, JournalKind, Platform, Target, Track, lock_for_update, state_path,
    };
    use super::*;
    use crate::gog::test_support::setup;

    const ID: u64 = 1094900565;
    const NEVER: &dyn Fn() -> bool = &|| false;

    /// A game folder with its state, under a canonical root.
    fn game(paths: &Paths, root: &Path, owner: Option<&str>, mid_update: bool) -> PathBuf {
        let folder = fs::canonicalize(root).unwrap().join("Games/RimWorld");
        fs::create_dir_all(&folder).unwrap();
        let target = Target {
            gog_id: ID,
            platform: Platform::Linux,
            language: "en-US".to_owned(),
            track: Track::Installer,
            products: Vec::new(),
        };
        let state = FolderState {
            committed: Some(Committed {
                gog_id: ID,
                platform: target.platform,
                language: target.language.clone(),
                track: target.track,
                products: Vec::new(),
                files: Vec::new(),
            }),
            journal: mid_update.then(|| Journal {
                operation: "op".to_owned(),
                kind: JournalKind::Update,
                target,
                packages: Vec::new(),
                attempted: Vec::new(),
            }),
            ..FolderState::new(folder.clone(), owner.map(str::to_owned))
        };
        FolderLock::exclusive(paths, &folder, NEVER)
            .unwrap()
            .write(&state)
            .unwrap();
        folder
    }

    fn owner(paths: &Paths, folder: &Path) -> Option<String> {
        FolderLock::shared(paths, folder, NEVER)
            .unwrap()
            .read()
            .unwrap()
            .unwrap()
            .owner
    }

    fn refusal(result: Result<Vec<InUseGuard>>) -> FolderError {
        result
            .unwrap_err()
            .downcast::<FolderError>()
            .expect("a typed refusal")
    }

    #[test]
    fn missing_paths_resolve_through_their_nearest_existing_ancestor() {
        let (root, _) = setup();
        let real = fs::canonicalize(root.path()).unwrap();
        fs::create_dir_all(real.join("a")).unwrap();
        std::os::unix::fs::symlink(real.join("a"), real.join("link")).unwrap();
        assert_eq!(resolve(&real.join("link/new/x")), real.join("a/new/x"));
        assert_eq!(resolve(&real.join("link/new/../y")), real.join("a/y"));
        assert_eq!(resolve(&real.join("a/../link")), real.join("a"));
    }

    #[test]
    fn an_owner_mid_update_is_refused() {
        let (root, paths) = setup();
        let folder = game(&paths, root.path(), Some("rimworld"), true);
        let error = refusal(protect(&paths, "rimworld", Some("1094900565"), &[], NEVER));
        assert_eq!(
            error.to_string(),
            "rimworld's game folder is mid-update. Finish it with wineo update rimworld."
        );
        // The refusal let go of the in-use lock.
        assert!(lock_for_update(&paths, &folder, "rimworld").is_ok());
    }

    #[test]
    fn an_owner_holds_the_in_use_lock_whatever_it_launches() {
        let (root, paths) = setup();
        let folder = game(&paths, root.path(), Some("rimworld"), false);
        let held = protect(&paths, "rimworld", Some("1094900565"), &[], NEVER).unwrap();
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].lock().folder(), folder);
        assert!(matches!(
            lock_for_update(&paths, &folder, "rimworld")
                .unwrap_err()
                .downcast_ref::<FolderError>(),
            Some(FolderError::InUse { .. })
        ));
        drop(held);
        assert!(lock_for_update(&paths, &folder, "rimworld").is_ok());
    }

    #[test]
    fn another_environment_is_checked_by_the_paths_it_names() {
        let (root, paths) = setup();
        let folder = game(&paths, root.path(), Some("rimworld"), true);
        let outside = fs::canonicalize(root.path()).unwrap();
        assert!(
            protect(&paths, "copy", None, &[outside.join("other")], NEVER)
                .unwrap()
                .is_empty()
        );
        for path in [folder.clone(), folder.join("Data/new.exe")] {
            let error = refusal(protect(&paths, "copy", None, &[path], NEVER));
            assert!(matches!(&error, FolderError::MidUpdate { name } if name == "rimworld"));
        }
    }

    #[test]
    fn an_update_in_progress_refuses_a_launch() {
        let (root, paths) = setup();
        let folder = game(&paths, root.path(), Some("rimworld"), false);
        let _updating = lock_for_update(&paths, &folder, "rimworld").unwrap();
        let error = refusal(protect(&paths, "rimworld", Some("1094900565"), &[], NEVER));
        assert_eq!(
            error.to_string(),
            "rimworld's game folder is being updated."
        );
    }

    #[test]
    fn a_lost_state_is_refused() {
        let (_root, paths) = setup();
        let error = refusal(protect(&paths, "rimworld", Some("1094900565"), &[], NEVER));
        assert_eq!(
            error.to_string(),
            "rimworld's GOG state is missing. Migrate it again with wineo import gog --game 1094900565, or stop tracking it with wineo set rimworld --unset gog_id."
        );
    }

    #[test]
    fn an_ownerless_folder_mid_update_says_to_migrate_again() {
        let (root, paths) = setup();
        let folder = game(&paths, root.path(), None, false);
        assert_eq!(
            protect(&paths, "copy", None, &[folder.join("x")], NEVER)
                .unwrap()
                .len(),
            1
        );
        game(&paths, root.path(), None, true);
        let error = refusal(protect(&paths, "copy", None, &[folder.join("x")], NEVER));
        assert_eq!(
            error.to_string(),
            "GOG game 1094900565's folder is mid-update. Migrate it again with wineo import gog --game 1094900565 and finish it."
        );
        assert_eq!(owner(&paths, &folder), None);
    }

    #[test]
    fn an_ownerless_folder_of_its_game_is_claimed_once() {
        let (root, paths) = setup();
        let folder = game(&paths, root.path(), None, false);
        let exe = folder.join("RimWorldLinux");
        protect(
            &paths,
            "rimworld",
            Some("1094900565"),
            std::slice::from_ref(&exe),
            NEVER,
        )
        .unwrap();
        assert_eq!(owner(&paths, &folder).as_deref(), Some("rimworld"));
        let written = fs::metadata(state_path(&paths, &folder)).unwrap().ino();
        protect(
            &paths,
            "rimworld",
            Some("1094900565"),
            std::slice::from_ref(&exe),
            NEVER,
        )
        .unwrap();
        assert_eq!(
            fs::metadata(state_path(&paths, &folder)).unwrap().ino(),
            written,
            "an owned folder is never written again"
        );
        let error = refusal(protect(&paths, "other", Some("1094900565"), &[exe], NEVER));
        assert!(matches!(error, FolderError::StateMissing { .. }));
        assert_eq!(owner(&paths, &folder).as_deref(), Some("rimworld"));
    }

    #[test]
    fn a_claimed_folder_mid_update_names_its_new_owner() {
        let (root, paths) = setup();
        let folder = game(&paths, root.path(), None, true);
        let error = refusal(protect(
            &paths,
            "rimworld",
            Some("1094900565"),
            std::slice::from_ref(&folder),
            NEVER,
        ));
        assert!(matches!(&error, FolderError::MidUpdate { name } if name == "rimworld"));
        assert_eq!(owner(&paths, &folder).as_deref(), Some("rimworld"));
    }

    #[test]
    fn racing_claims_agree() {
        let (root, paths) = setup();
        let folder = game(&paths, root.path(), None, false);
        let barrier = Arc::new(Barrier::new(2));
        let racers: Vec<_> = (0..2)
            .map(|_| {
                let (paths, folder, barrier) = (paths.clone(), folder.clone(), barrier.clone());
                thread::spawn(move || {
                    barrier.wait();
                    protect(&paths, "rimworld", Some("1094900565"), &[folder], NEVER)
                        .map(|held| held.len())
                })
            })
            .collect();
        for racer in racers {
            assert_eq!(racer.join().unwrap().unwrap(), 1);
        }
        assert_eq!(owner(&paths, &folder).as_deref(), Some("rimworld"));
    }

    #[test]
    fn only_an_ancestors_broken_state_refuses() {
        let (root, paths) = setup();
        let real = fs::canonicalize(root.path()).unwrap();
        let broken = real.join("Broken");
        let elsewhere = real.join("Elsewhere");
        fs::create_dir_all(&broken).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        fs::create_dir_all(paths.gog_folders_dir()).unwrap();
        fs::write(state_path(&paths, &broken), "not = [valid").unwrap();
        assert!(
            protect(&paths, "copy", None, &[elsewhere.join("x.exe")], NEVER)
                .unwrap()
                .is_empty()
        );
        let error = refusal(protect(
            &paths,
            "copy",
            None,
            &[broken.join("x.exe")],
            NEVER,
        ));
        assert!(
            matches!(&error, FolderError::UnreadableFolder { folder, .. } if *folder == broken)
        );
        assert!(
            error.to_string().starts_with(&format!(
                "The GOG state for {} can't be checked: ",
                broken.display()
            )),
            "{error}"
        );
    }

    #[test]
    fn precheck_refuses_what_protect_would_without_claiming_or_holding() {
        let (root, paths) = setup();
        let lost =
            refusal(precheck(&paths, "rimworld", Some("1094900565"), &[]).map(|()| Vec::new()));
        assert!(matches!(lost, FolderError::StateMissing { .. }));

        let folder = game(&paths, root.path(), None, false);
        let written = fs::metadata(state_path(&paths, &folder)).unwrap().ino();
        let touched = [folder.join("RimWorldLinux")];
        precheck(&paths, "rimworld", Some("1094900565"), &touched).unwrap();
        assert_eq!(owner(&paths, &folder), None);
        assert_eq!(
            fs::metadata(state_path(&paths, &folder)).unwrap().ino(),
            written
        );
        let updating = lock_for_update(&paths, &folder, "rimworld").unwrap();
        let error = refusal(
            precheck(&paths, "rimworld", Some("1094900565"), &touched).map(|()| Vec::new()),
        );
        assert!(matches!(error, FolderError::Updating { .. }));
        drop(updating);

        game(&paths, root.path(), None, true);
        let error = refusal(
            precheck(&paths, "rimworld", Some("1094900565"), &touched).map(|()| Vec::new()),
        );
        assert!(matches!(&error, FolderError::MidUpdate { name } if name == "rimworld"));
        let error = refusal(precheck(&paths, "copy", None, &touched).map(|()| Vec::new()));
        assert!(matches!(
            error,
            FolderError::OwnerlessMidUpdate { gog_id: ID }
        ));
        assert_eq!(owner(&paths, &folder), None);
        assert!(lock_for_update(&paths, &folder, "rimworld").is_ok());
    }

    #[test]
    fn a_portable_gog_id_names_no_game() {
        let (root, _) = setup();
        let home = root.path().to_owned();
        let portable = Paths::select(
            move |key| (key == "HOME").then(|| home.clone().into_os_string()),
            Some(&root.path().join("usb")),
        )
        .unwrap();
        assert!(
            protect(&portable, "rimworld", Some("1094900565"), &[], NEVER)
                .unwrap()
                .is_empty()
        );
    }
}
