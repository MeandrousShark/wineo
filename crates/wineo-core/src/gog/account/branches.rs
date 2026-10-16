//! Private-branch passwords, kept beside the GOG login: one keyring item per
//! account, game, and branch (`account=gog-branch`, attributes `user`, `game`
//! and `branch`), or, with no keyring, one private file holding all of them.
//! Like the login, a password goes to the keyring when there is one and the
//! file's copy is dropped then, so a stale copy can't stay in plain text.
//!
//! A password belongs to the account that saved it, and only that account's
//! are read, listed, sent, or compared. So a logout or switch whose wipe
//! couldn't reach the keyring (a dropped connection answers like no keyring)
//! leaves nothing another account can use. What belongs to another account,
//! or to none (an item from before items had an owner), is removed by the
//! wipe and by any write here, which all happen under the credentials lock.
//!
//! When a keyring copy and a file copy of the same password both exist, the
//! file's is the newer: it was written while the keyring was unavailable,
//! and a later keyring save drops it.

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

/// Game id to branch name to password, for one account.
type Games = BTreeMap<String, BTreeMap<String, String>>;

/// Account id to that account's passwords.
#[derive(Default, PartialEq, Eq, Serialize, Deserialize)]
struct Passwords(BTreeMap<String, Games>);

/// What the file holds, and whether it parsed. A file that doesn't (one
/// from before passwords had owners is shaped differently) has nothing
/// usable, and costs only entering them again.
fn read_file(paths: &Paths) -> Result<(Passwords, bool)> {
    let path = file(paths);
    let Some(text) = files::read_to_string_optional(&path)
        .context(|| format!("failed to read {}", path.display()))?
    else {
        return Ok((Passwords::default(), true));
    };
    // Not shown: what's in it is a password.
    Ok(match serde_json::from_str(&text) {
        Ok(passwords) => (passwords, true),
        Err(_) => (Passwords::default(), false),
    })
}

fn write_file(paths: &Paths, passwords: &Passwords) -> Result<()> {
    let path = file(paths);
    if passwords.0.values().all(Games::is_empty) {
        key_store::remove_key_file(&path)?;
        return Ok(());
    }
    let text = serde_json::to_string(passwords).expect("passwords serialize");
    key_store::write_private(&path, &text)?;
    // What an earlier crashed save left holds passwords too.
    key_store::sweep_leftovers(&path)
}

/// Runs `edit` on `user`'s passwords in the file. Whatever else is in it,
/// another account's or what isn't readable, is dropped, and so are a
/// crashed save's leftovers. Only for a caller holding the credentials lock.
fn edit_file<T>(paths: &Paths, user: &str, edit: impl FnOnce(&mut Games) -> T) -> Result<T> {
    key_store::sweep_leftovers(&file(paths))?;
    let (mut all, readable) = read_file(paths)?;
    let mut mine = all.0.remove(user).unwrap_or_default();
    let foreign = !readable || !all.0.is_empty();
    let before = mine.clone();
    let result = edit(&mut mine);
    mine.retain(|_, branches| !branches.is_empty());
    if foreign || mine != before {
        let mut kept = Passwords::default();
        if !mine.is_empty() {
            kept.0.insert(user.to_owned(), mine);
        }
        write_file(paths, &kept)?;
    }
    Ok(result)
}

fn keyring_error(error: KeyringError) -> Option<Error> {
    match error {
        KeyringError::Unavailable => None,
        KeyringError::Failed(reason) => Some(Error::Keyring(reason)),
    }
}

/// Removes what other accounts saved from the keyring, as far as it can be
/// reached. Cleanup, so a keyring that can't be is not a failure: nothing
/// else here ever reads those items.
fn purge_keyring(keyring: &dyn Keyring, user: &str) {
    let _ = keyring.delete_branches_not_of(user);
}

/// The password `user` saved for `branch` of `game`.
pub(crate) fn find(
    paths: &Paths,
    keyring: &dyn Keyring,
    user: &str,
    game: u64,
    branch: &str,
) -> Result<Option<Secret>> {
    let (all, _) = read_file(paths)?;
    let from_file = all
        .0
        .get(user)
        .and_then(|games| games.get(&game.to_string()))
        .and_then(|branches| branches.get(branch))
        .filter(|password| !password.is_empty());
    if let Some(password) = from_file {
        return Ok(Some(Secret::new(password.clone())));
    }
    match keyring.get_branch(user, game, branch) {
        Ok(Some(password)) if !password.is_empty() => Ok(Some(Secret::new(password))),
        Ok(_) => Ok(None),
        Err(error) => keyring_error(error).map_or(Ok(None), Err),
    }
}

/// Every branch of `game` with a password `user` saved, by branch name.
pub(crate) fn list(
    paths: &Paths,
    keyring: &dyn Keyring,
    user: &str,
    game: u64,
) -> Result<Vec<(String, Secret)>> {
    let mut found: BTreeMap<String, Secret> = BTreeMap::new();
    match keyring.list_branches(user, game) {
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
    let (all, _) = read_file(paths)?;
    if let Some(branches) = all
        .0
        .get(user)
        .and_then(|games| games.get(&game.to_string()))
    {
        for (branch, password) in branches.iter().filter(|(_, password)| !password.is_empty()) {
            found.insert(branch.clone(), Secret::new(password.clone()));
        }
    }
    Ok(found.into_iter().collect())
}

/// Saves the password for `user`, replacing the one for that game and
/// branch.
pub(crate) fn save(
    paths: &Paths,
    keyring: &dyn Keyring,
    user: &str,
    game: u64,
    branch: &str,
    password: &Secret,
) -> Result<Saved> {
    match keyring.set_branch(user, game, branch, password.expose()) {
        Ok(()) => {
            // The keyring has it now; a copy left in the file would outlive
            // a later removal from the keyring.
            edit_file(paths, user, |mine| {
                if let Some(branches) = mine.get_mut(&game.to_string()) {
                    branches.remove(branch);
                }
            })?;
            purge_keyring(keyring, user);
            Ok(Saved::Keyring)
        }
        Err(KeyringError::Unavailable) => {
            edit_file(paths, user, |mine| {
                mine.entry(game.to_string())
                    .or_default()
                    .insert(branch.to_owned(), password.expose().to_owned());
            })?;
            Ok(Saved::File(file(paths)))
        }
        Err(KeyringError::Failed(reason)) => Err(Error::Keyring(reason)),
    }
}

/// Removes `user`'s saved password if it is still `refused`, the one that
/// was sent and not accepted, checking the keyring's copy and the file's
/// each by itself. One saved since, by another process, stays. Returns
/// whether anything was removed.
pub(crate) fn remove_if(
    paths: &Paths,
    keyring: &dyn Keyring,
    user: &str,
    game: u64,
    branch: &str,
    refused: &Secret,
) -> Result<bool> {
    // Whichever copy `find` would send is the one compared.
    if find(paths, keyring, user, game, branch)?.is_none_or(|saved| saved != *refused) {
        edit_file(paths, user, |_| ())?;
        purge_keyring(keyring, user);
        return Ok(false);
    }
    let from_keyring = match keyring.delete_branch(user, game, branch) {
        Ok(deleted) => deleted,
        Err(error) => match keyring_error(error) {
            Some(error) => return Err(error),
            None => false,
        },
    };
    let from_file = edit_file(paths, user, |mine| {
        mine.get_mut(&game.to_string())
            .is_some_and(|branches| branches.remove(branch).is_some())
    })?;
    purge_keyring(keyring, user);
    Ok(from_file || from_keyring)
}

/// Removes every saved password of every account and game. Returns whether
/// there were any. A keyring that can't be reached is not a failure: what
/// it holds is owned, and no other account reads it.
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
