//! Private-branch passwords, kept beside the GOG login: one keyring item per
//! game and branch (`account=gog-branch`, attributes `game` and `branch`),
//! or, with no keyring, one private file holding all of them. Like the login,
//! a password goes to the keyring when there is one and the file is cleared
//! then, so a stale copy can't stay in plain text.
//!
//! Nothing here takes the credentials lock: the callers that change things
//! hold it, which is also what lets a logout wipe them in the same step as
//! the login.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use wineo_gog::Secret;

use crate::error::{Error, IoContext, Result};
use crate::files;
use crate::key_store::{self, Keyring, KeyringError, Saved};
use crate::paths::Paths;

const FILE_NAME: &str = "gog-branches.key";

/// The plain text fallback, always on this machine, never in a portable
/// folder.
pub(crate) fn file(paths: &Paths) -> PathBuf {
    paths.host_data().join(FILE_NAME)
}

/// Game id to branch name to password.
#[derive(Default, Serialize, Deserialize)]
struct Passwords(BTreeMap<String, BTreeMap<String, String>>);

fn read_file(paths: &Paths) -> Result<Passwords> {
    let path = file(paths);
    let Some(text) = files::read_to_string_optional(&path)
        .context(|| format!("failed to read {}", path.display()))?
    else {
        return Ok(Passwords::default());
    };
    // Not shown: what's in it is a password. Starting over only costs
    // entering them again.
    Ok(serde_json::from_str(&text).unwrap_or_default())
}

fn write_file(paths: &Paths, passwords: &Passwords) -> Result<()> {
    let path = file(paths);
    if passwords.0.is_empty() {
        key_store::remove_key_file(&path)?;
        return Ok(());
    }
    let text = serde_json::to_string(passwords).expect("passwords serialize");
    key_store::write_private(&path, &text)?;
    // What an earlier crashed save left holds passwords too.
    key_store::sweep_leftovers(&path)
}

fn keyring_error(error: KeyringError) -> Option<Error> {
    match error {
        KeyringError::Unavailable => None,
        KeyringError::Failed(reason) => Some(Error::Keyring(reason)),
    }
}

/// The saved password for `branch` of `game`.
pub(crate) fn find(
    paths: &Paths,
    keyring: &dyn Keyring,
    game: u64,
    branch: &str,
) -> Result<Option<Secret>> {
    match keyring.get_branch(game, branch) {
        Ok(Some(password)) if !password.is_empty() => return Ok(Some(Secret::new(password))),
        Ok(_) => {}
        Err(error) => {
            if let Some(error) = keyring_error(error) {
                return Err(error);
            }
        }
    }
    Ok(read_file(paths)?
        .0
        .get(&game.to_string())
        .and_then(|branches| branches.get(branch))
        .filter(|password| !password.is_empty())
        .map(|password| Secret::new(password.clone())))
}

/// Every branch of `game` with a saved password, by branch name.
pub(crate) fn list(
    paths: &Paths,
    keyring: &dyn Keyring,
    game: u64,
) -> Result<Vec<(String, Secret)>> {
    let mut found: BTreeMap<String, Secret> = BTreeMap::new();
    match keyring.list_branches(game) {
        Ok(items) => {
            for (branch, password) in items
                .into_iter()
                .filter(|(_, password)| !password.is_empty())
            {
                found.insert(branch, Secret::new(password));
            }
        }
        Err(error) => {
            if let Some(error) = keyring_error(error) {
                return Err(error);
            }
        }
    }
    if let Some(branches) = read_file(paths)?.0.get(&game.to_string()) {
        for (branch, password) in branches.iter().filter(|(_, password)| !password.is_empty()) {
            found
                .entry(branch.clone())
                .or_insert_with(|| Secret::new(password.clone()));
        }
    }
    Ok(found.into_iter().collect())
}

/// Saves the password, replacing the one for that game and branch.
pub(crate) fn save(
    paths: &Paths,
    keyring: &dyn Keyring,
    game: u64,
    branch: &str,
    password: &Secret,
) -> Result<Saved> {
    match keyring.set_branch(game, branch, password.expose()) {
        Ok(()) => {
            // The keyring has it now; a copy left in the file would outlive
            // a later removal from the keyring.
            drop_from_file(paths, game, branch)?;
            Ok(Saved::Keyring)
        }
        Err(KeyringError::Unavailable) => {
            let mut passwords = read_file(paths)?;
            passwords
                .0
                .entry(game.to_string())
                .or_default()
                .insert(branch.to_owned(), password.expose().to_owned());
            write_file(paths, &passwords)?;
            Ok(Saved::File(file(paths)))
        }
        Err(KeyringError::Failed(reason)) => Err(Error::Keyring(reason)),
    }
}

fn drop_from_file(paths: &Paths, game: u64, branch: &str) -> Result<bool> {
    // Whether or not the branch is in the file, a crashed save may have left
    // its passwords beside it.
    key_store::sweep_leftovers(&file(paths))?;
    let mut passwords = read_file(paths)?;
    let key = game.to_string();
    let Some(branches) = passwords.0.get_mut(&key) else {
        return Ok(false);
    };
    let had = branches.remove(branch).is_some();
    if branches.is_empty() {
        passwords.0.remove(&key);
    }
    if had {
        write_file(paths, &passwords)?;
    }
    Ok(had)
}

/// Removes the saved password if it is still `refused`, the one that was
/// sent and not accepted. One saved since, by another process, stays.
/// Returns whether anything was removed.
pub(crate) fn remove_if(
    paths: &Paths,
    keyring: &dyn Keyring,
    game: u64,
    branch: &str,
    refused: &Secret,
) -> Result<bool> {
    if find(paths, keyring, game, branch)?.is_none_or(|saved| saved != *refused) {
        // Still tidy up after a crashed save.
        key_store::sweep_leftovers(&file(paths))?;
        return Ok(false);
    }
    let from_keyring = match keyring.delete_branch(game, branch) {
        Ok(deleted) => deleted,
        Err(error) => match keyring_error(error) {
            Some(error) => return Err(error),
            None => false,
        },
    };
    Ok(drop_from_file(paths, game, branch)? || from_keyring)
}

/// Removes every saved password of every game. Returns whether there were
/// any.
pub(crate) fn remove_all(paths: &Paths, keyring: &dyn Keyring) -> Result<bool> {
    let from_keyring = match keyring.delete_all_branches() {
        Ok(deleted) => deleted,
        Err(error) => match keyring_error(error) {
            Some(error) => return Err(error),
            None => false,
        },
    };
    let path = file(paths);
    // Also sweeps the temp files a crashed save left, which hold passwords.
    let from_file = key_store::remove_key_file(&path)?;
    Ok(from_keyring || from_file)
}
