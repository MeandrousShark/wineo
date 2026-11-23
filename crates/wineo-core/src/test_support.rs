//! Helpers the crates' tests share.

use std::fs;
use std::io;

use crate::paths::Paths;

/// Replaces `name` the way `rm` and then making it again would, as far as
/// its identity goes: a new config file with the same text, and a new
/// library entry with a new id from the same calls `rm` and creation make.
/// No identity held for the old one is current, whichever kind it is.
///
/// It takes no wineo lock, so a test can replace an environment at a hook
/// where the code under test holds one that `rm` would wait for. Art and
/// other state stay where they are.
///
/// An environment that had an id must end with a different one, or this
/// fails: otherwise a test holding the old id would be refused only for
/// the config swap, and would no longer be testing the id at all. One with
/// no id, or a library that can't be read, is replaced as `rm` and creation
/// would, warnings and all.
pub fn replace_environment(paths: &Paths, name: &str) -> io::Result<()> {
    let before = crate::library::read_id(paths, name).ok().flatten();
    let config = paths.environment_config(name);
    // Staged beside it, so the new file can't reuse the old one's inode.
    let staged = config.with_extension("replacing");
    fs::copy(&config, &staged)?;
    let _ = crate::library::remove(paths, name);
    fs::rename(&staged, &config)?;
    let _ = crate::library::fresh(paths, name);
    if let Some(before) = before {
        let after = crate::library::read_id(paths, name).ok().flatten();
        if after.is_none_or(|after| after == before) {
            return Err(io::Error::other(format!(
                "replacing {name} left its library entry without a new id ({after:?})"
            )));
        }
    }
    Ok(())
}
