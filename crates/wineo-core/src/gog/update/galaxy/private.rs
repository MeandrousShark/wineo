//! Private branches. GOG lists a private branch's builds only when the
//! request carries its password, and answers a wrong one like none at all
//! (probed 2026-10-16: same status, same public builds, no error field), so
//! a refused password can't be told from a branch that isn't there. A
//! password counts as accepted when its branch shows up in the list fetched
//! with it, and not before it is saved.
//!
//! The password for a branch is the one entered for this run, else the one
//! saved for the game and branch. It reaches `wineo-gog` only as a request
//! parameter.

use anyhow::{Result, anyhow};
use wineo_gog::{Builds, Secret};

use super::build;
use super::follow::Choice;
use super::install::GalaxyError;
use super::record::Follow;
use crate::gog::account::{CallError, Session};
use crate::key_store::Saved;

/// Which password a builds request carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::gog::update) enum Sent {
    Nothing,
    /// One entered for this run, saved once GOG accepts it.
    Entered,
    /// One saved earlier.
    Saved,
}

/// A game's builds, and what was sent to get them.
pub(in crate::gog::update) struct Fetched {
    pub(in crate::gog::update) builds: Builds,
    pub(in crate::gog::update) sent: Sent,
    entered: Option<Secret>,
}

/// The branch whose password a choice's builds request carries, and whether
/// that branch has to be listed for the choice to make sense. Returning to
/// the default branch needs no password. A build can be on the branch
/// followed, so its password is sent when there is one, but a build on a
/// public branch doesn't need it.
pub(super) fn wanted<'a>(choice: &'a Choice, from: &'a Follow) -> (Option<&'a str>, bool) {
    match choice {
        Choice::Keep | Choice::Latest => (from.branch.as_deref(), from.branch.is_some()),
        Choice::Branch(branch) => (Some(branch), true),
        Choice::DefaultBranch => (None, false),
        Choice::Build(_) => (from.branch.as_deref(), false),
    }
}

/// The builds GOG lists for a game, with the password for `branch`: the one
/// `entered`, else the one saved.
pub(super) fn fetch(
    session: &mut Session,
    gog_id: u64,
    branch: Option<&str>,
    entered: Option<&Secret>,
) -> Result<Fetched> {
    fetch_call(session, gog_id, branch, entered).map_err(|error| match error {
        CallError::Account(error) => anyhow::Error::from(error),
        CallError::Request(error) => anyhow::Error::from(error),
    })
}

/// `fetch`, its failures kept as the session reports them.
pub(in crate::gog::update) fn fetch_call(
    session: &mut Session,
    gog_id: u64,
    branch: Option<&str>,
    entered: Option<&Secret>,
) -> Result<Fetched, CallError> {
    let (password, sent) = match branch {
        None => (None, Sent::Nothing),
        Some(branch) => match entered {
            Some(entered) => (Some(entered.clone()), Sent::Entered),
            None => match session
                .branch_password(gog_id, branch)
                .map_err(CallError::Account)?
            {
                Some(saved) => (Some(saved), Sent::Saved),
                None => (None, Sent::Nothing),
            },
        },
    };
    let builds = session
        .call(|client, access| client.windows_builds(gog_id, Some(access), password.as_ref()))?;
    Ok(Fetched {
        builds,
        sent,
        entered: password.filter(|_| sent == Sent::Entered),
    })
}

/// The game a password is for, and how to name it in a command.
pub(super) struct Game<'a> {
    pub(super) title: &'a str,
    pub(super) gog_id: u64,
    /// The environment, when there is one.
    pub(super) name: Option<&'a str>,
}

impl Game<'_> {
    fn command(&self, branch: &str) -> String {
        let branch = shell_word(branch);
        match self.name {
            Some(name) => format!("wineo update {name} --branch {branch}"),
            None => format!("wineo import gog --game {} --branch {branch}", self.gog_id),
        }
    }
}

/// `word` as one shell word, quoted only when it has to be.
pub(crate) fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-+=/@:,".contains(&byte));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

impl Fetched {
    /// A game GOG has no content system for.
    pub(in crate::gog::update) fn empty() -> Self {
        Self {
            builds: Builds {
                items: Vec::new(),
                has_private_branches: false,
            },
            sent: Sent::Nothing,
            entered: None,
        }
    }

    /// Whether `branch` has a generation 2 build in the list.
    pub(super) fn lists(&self, branch: &str) -> bool {
        build::latest(&self.builds, Some(branch)).is_some()
    }

    /// Whether the list lacks `branch`, GOG says some branches are private,
    /// and no password was sent for it: the case where asking for one can
    /// help.
    pub(in crate::gog::update) fn needs_password(&self, branch: &str) -> bool {
        self.sent == Sent::Nothing && self.builds.has_private_branches && !self.lists(branch)
    }

    /// Whether a password was sent for `branch` and the list still lacks it,
    /// which is how GOG refuses one.
    pub(in crate::gog::update) fn refused(&self, branch: &str) -> bool {
        self.sent != Sent::Nothing && !self.lists(branch)
    }

    /// Settles a branch the caller needs. When it is listed, an entered
    /// password is saved now that GOG has accepted it, and a failure to save
    /// is the warning returned. When it isn't, and a password was sent, the
    /// saved password is removed and the refusal is the error; with none
    /// sent and private branches about, the password is what's missing.
    /// Otherwise the caller's own "no such branch" follows. `read_only`
    /// writes nothing.
    pub(super) fn require(
        &self,
        session: &mut Session,
        who: &Game,
        branch: &str,
        read_only: bool,
    ) -> Result<Option<String>> {
        if self.lists(branch) {
            return Ok(self.accept(session, who, branch, read_only));
        }
        let refused = || GalaxyError::Refused {
            title: who.title.to_owned(),
            branch: branch.to_owned(),
        };
        match self.sent {
            Sent::Nothing if self.builds.has_private_branches => Err(GalaxyError::NeedsPassword {
                title: who.title.to_owned(),
                branch: branch.to_owned(),
                command: who.command(branch),
            }
            .into()),
            Sent::Nothing => Ok(None),
            Sent::Entered | Sent::Saved => {
                if !read_only && let Err(error) = session.forget_branch_password(who.gog_id, branch)
                {
                    return Err(anyhow!(
                        "{} Couldn't remove its saved password: {error}",
                        refused()
                    ));
                }
                Err(refused().into())
            }
        }
    }

    /// Saves a password entered for `branch` after GOG listed it. The
    /// warning says it couldn't be kept.
    pub(super) fn accept(
        &self,
        session: &mut Session,
        who: &Game,
        branch: &str,
        read_only: bool,
    ) -> Option<String> {
        let password = self.entered.as_ref().filter(|_| !read_only)?;
        match session.save_branch_password(who.gog_id, branch, password) {
            // Like the login, a password with no keyring to hold it says so.
            Ok(Saved::File(path)) => Some(format!(
                "No keyring, so the password for branch {branch} is stored in plain text in {}.",
                path.display()
            )),
            Ok(Saved::Keyring) => None,
            Err(error) => Some(format!(
                "Couldn't save the password for branch {branch} of {}: {error}. It will be asked for again.",
                who.title
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names_are_quoted_only_when_a_shell_needs_it() {
        assert_eq!(shell_word("beta"), "beta");
        assert_eq!(shell_word("1.4-rc_2"), "1.4-rc_2");
        assert_eq!(shell_word("Legacy (64-bit)"), "'Legacy (64-bit)'");
        assert_eq!(shell_word("it's"), "'it'\\''s'");
        assert_eq!(shell_word(""), "''");
    }

    #[test]
    fn a_choice_names_the_branch_whose_password_it_needs() {
        let on_beta = Follow {
            branch: Some("beta".to_owned()),
            pinned: None,
        };
        let on_default = Follow::default();
        assert_eq!(wanted(&Choice::Keep, &on_beta), (Some("beta"), true));
        assert_eq!(wanted(&Choice::Latest, &on_beta), (Some("beta"), true));
        assert_eq!(wanted(&Choice::Keep, &on_default), (None, false));
        assert_eq!(
            wanted(&Choice::Branch("gamma".to_owned()), &on_beta),
            (Some("gamma"), true)
        );
        assert_eq!(wanted(&Choice::DefaultBranch, &on_beta), (None, false));
        assert_eq!(
            wanted(&Choice::Build("1".to_owned()), &on_beta),
            (Some("beta"), false)
        );
    }
}
