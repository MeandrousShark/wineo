//! The Epic account, as Legendary keeps it in its own state folder. Who is
//! logged in is only ever read from `status --offline --json`, since
//! `auth` exits 0 whether or not it worked.

use std::fmt;

use super::legendary::Legendary;
use super::{EpicLock, Error};
use crate::cancel::Token;
use crate::error::IoContext;
use crate::paths::Paths;

/// What `status` names as the account when nobody is logged in.
const LOGGED_OUT: &str = "<not logged in>";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    LoggedIn { account: String },
    LoggedOut,
}

/// The code from Epic's login page. It works once, but it is a login while
/// it does, so it never reaches output, errors, logs, or `Debug`.
#[derive(Clone)]
pub struct LoginCode(String);

impl fmt::Debug for LoginCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl LoginCode {
    /// What the user pasted: the page's whole JSON, or only the code. A
    /// failure never repeats what was pasted.
    pub fn parse(pasted: &str) -> Result<Self, Error> {
        let pasted = pasted.trim();
        let code = if pasted.starts_with('{') {
            let page: serde_json::Value =
                serde_json::from_str(pasted).map_err(|_| Error::NotACode)?;
            page.get("authorizationCode")
                .and_then(serde_json::Value::as_str)
                .ok_or(Error::NotACode)?
                .to_owned()
        } else {
            pasted.trim_matches('"').to_owned()
        };
        // Only letters and digits, so it can never read as an option.
        if code.is_empty()
            || code.len() > 256
            || !code.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(Error::NotACode);
        }
        Ok(Self(code))
    }

    fn reveal(&self) -> &str {
        &self.0
    }
}

/// Who Legendary has logged in, read without asking Epic.
pub fn status(legendary: &Legendary, token: &Token) -> Result<Status, Error> {
    const COMMAND: &str = "status --offline --json";
    let output = legendary.run(&["status", "--offline", "--json"], None, token)?;
    let unreadable = |problem: &str| Error::Output {
        command: COMMAND.to_owned(),
        problem: problem.to_owned(),
    };
    let value: serde_json::Value =
        serde_json::from_str(&output.stdout).map_err(|_| unreadable("it isn't JSON"))?;
    let account = value
        .get("account")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| unreadable("`account` is missing or isn't text"))?;
    Ok(match account {
        LOGGED_OUT => Status::LoggedOut,
        "" => return Err(unreadable("`account` is empty")),
        account => Status::LoggedIn {
            account: account.to_owned(),
        },
    })
}

/// Logs in with `code` and returns the account now logged in. A saved
/// login is deleted first, since `auth --code` keeps it otherwise.
pub fn login(legendary: &Legendary, code: &LoginCode, token: &Token) -> Result<String, Error> {
    let _lock = EpicLock::acquire(legendary.paths())?;
    log_out_locked(legendary, token)?;
    let output = legendary.run(&["auth", "--code"], Some(code.reveal()), token)?;
    match status(legendary, token)? {
        Status::LoggedIn { account } => Ok(account),
        Status::LoggedOut => Err(Error::LoginRejected {
            detail: legendary_error(&output.stderr),
        }),
    }
}

/// Logs out and removes the cached owned list. Returns whether someone
/// was logged in. Installed games keep their records.
pub fn logout(legendary: &Legendary, token: &Token) -> Result<bool, Error> {
    let _lock = EpicLock::acquire(legendary.paths())?;
    let was = log_out_locked(legendary, token)?;
    super::owned::remove_cache(legendary.paths())?;
    Ok(was)
}

/// Logs out when no Legendary can be run: removes Legendary's saved login
/// from its state folder, which is all its own logout does, and the cached
/// owned list. Returns whether a login was saved. A symlink there is
/// removed, never followed.
pub fn logout_without_legendary(paths: &Paths) -> Result<bool, Error> {
    let _lock = EpicLock::acquire(paths)?;
    let user = super::state_dir(paths).join("user.json");
    // `remove_file` takes a symlink away itself and refuses a folder.
    let was = crate::files::remove_if_present(&user)
        .context(|| format!("failed to remove {}", user.display()))?;
    super::owned::remove_cache(paths)?;
    Ok(was)
}

fn log_out_locked(legendary: &Legendary, token: &Token) -> Result<bool, Error> {
    if status(legendary, token)? == Status::LoggedOut {
        return Ok(false);
    }
    legendary.run(&["auth", "--delete"], None, token)?;
    match status(legendary, token)? {
        Status::LoggedOut => Ok(true),
        Status::LoggedIn { account } => Err(Error::StillLoggedIn { account }),
    }
}

/// The first error Legendary logged, without its `[module] ERROR:` tag.
fn legendary_error(stderr: &str) -> Option<String> {
    let line = stderr.lines().find(|line| line.contains("ERROR: "))?;
    let (_, message) = line.split_once("ERROR: ")?;
    super::run::last_line(message)
}

#[cfg(test)]
mod tests;
