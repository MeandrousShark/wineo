//! The GOG login: one secret, `{refresh_token, user_id}`, kept like the API
//! keys (keyring first, then a private file). The access token only ever
//! lives in memory.
//!
//! Login, logout, and renewal all take one host-wide lock. A renewal sends
//! its request without the lock, then saves only if the stored login is
//! still the one it started from, so a stale renewal can't overwrite a newer
//! login or bring one back after a logout.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

use rustix::fs::{FlockOperation, flock};
use serde::{Deserialize, Serialize};
use wineo_gog::{Class, Client, ErrorKind, PasteError, Secret, Tokens, UserId};

use super::owned;
use crate::error::IoContext;
use crate::key_store::{self, KeySource, KeySpec, Keyring, Removed, Saved};
use crate::paths::Paths;

/// No variable overrides the login: GOG may replace the refresh token on
/// every renewal, which a variable couldn't keep up with.
pub const KEY_SPEC: KeySpec = KeySpec {
    env_var: None,
    account: "gog",
    file_name: "gog.key",
    label: "wineo GOG login",
};

/// How long before the access token expires that it is renewed.
const RENEW_EARLY: Duration = Duration::from_secs(60);

/// A renewal that keeps finding a newer login gives up after this many.
const RENEW_TRIES: usize = 3;

/// The saved login.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredLogin {
    pub refresh_token: String,
    pub user_id: UserId,
}

impl fmt::Debug for StoredLogin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredLogin")
            .field("refresh_token", &"<redacted>")
            .field("user_id", &self.user_id)
            .finish()
    }
}

impl StoredLogin {
    fn from_tokens(tokens: &Tokens) -> Self {
        Self {
            refresh_token: tokens.refresh_token.expose().to_owned(),
            user_id: tokens.user_id,
        }
    }

    fn to_secret(&self) -> String {
        serde_json::to_string(self).expect("a login serializes")
    }

    fn from_secret(secret: &str) -> Result<Self, AccountError> {
        serde_json::from_str::<Self>(secret)
            .ok()
            .filter(|login| !login.refresh_token.is_empty())
            .ok_or(AccountError::Unreadable)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    /// GOG refused the saved login, which has been cleared, or it was
    /// cleared by a logout while this was running.
    #[error("Log in to GOG again: wineo import gog --login.")]
    LoggedOut,
    /// GOG refused the code from a login.
    #[error("GOG didn't accept that login. Log in again and paste the new address.")]
    CodeRejected,
    /// The saved login is kept; trying again later may work.
    #[error("{0}")]
    Temporary(wineo_gog::Error),
    #[error("{0}")]
    Gog(wineo_gog::Error),
    #[error("{0}")]
    Paste(PasteError),
    /// What's saved isn't a login wineo wrote. It is never shown.
    #[error("The saved GOG login can't be read. Log in again: wineo import gog --login.")]
    Unreadable,
    #[error("The GOG login changed while it was being renewed. Try again.")]
    Changed,
    #[error(transparent)]
    Store(#[from] crate::Error),
}

impl AccountError {
    /// Whether trying again later may work without logging in again.
    pub fn is_temporary(&self) -> bool {
        matches!(self, Self::Temporary(_) | Self::Changed)
    }
}

/// Why an authenticated request failed: the login, or the request itself.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error(transparent)]
    Account(AccountError),
    #[error("{0}")]
    Request(wineo_gog::Error),
}

/// Classifies a token endpoint failure, for a renewal.
fn renewal_error(error: wineo_gog::Error) -> AccountError {
    match error.class() {
        Class::LoggedOut => AccountError::LoggedOut,
        Class::Temporary => AccountError::Temporary(error),
        Class::Malformed | Class::Other => AccountError::Gog(error),
    }
}

/// The saved login, from the keyring or the key file.
pub fn stored_login(
    paths: &Paths,
    keyring: &dyn Keyring,
) -> Result<Option<(StoredLogin, KeySource)>, AccountError> {
    let Some(key) = key_store::find_key(&KEY_SPEC, paths, |_| None, keyring)? else {
        return Ok(None);
    };
    Ok(Some((StoredLogin::from_secret(&key.value)?, key.source)))
}

fn saved_login(paths: &Paths, keyring: &dyn Keyring) -> Result<Option<StoredLogin>, AccountError> {
    stored_login(paths, keyring).map(|found| found.map(|(login, _)| login))
}

fn save(paths: &Paths, keyring: &dyn Keyring, login: &StoredLogin) -> crate::Result<Saved> {
    key_store::save_key(&KEY_SPEC, paths, keyring, &login.to_secret())
}

/// Holds the host-wide GOG credentials lock until dropped.
struct CredentialsLock {
    file: File,
}

impl CredentialsLock {
    fn acquire(paths: &Paths) -> crate::Result<Self> {
        let path = paths.gog_credentials_lock();
        let dir = path.parent().expect("the lock has a parent");
        fs::create_dir_all(dir).context(|| format!("failed to create {}", dir.display()))?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .context(|| format!("failed to open {}", path.display()))?;
        flock(&file, FlockOperation::LockExclusive)
            .map_err(std::io::Error::from)
            .context(|| format!("failed to lock {}", path.display()))?;
        Ok(Self { file })
    }
}

impl Drop for CredentialsLock {
    fn drop(&mut self) {
        // A forked child must not extend this holder's lifetime.
        let _ = flock(&self.file, FlockOperation::Unlock);
    }
}

#[derive(Debug)]
pub struct LoggedIn<'a> {
    pub user_id: UserId,
    pub saved: Saved,
    /// Ready to use, with the access token from the login.
    pub session: Session<'a>,
}

/// Logs in with what the user pasted after GOG's login page: trades the code
/// for tokens, then saves the login, replacing any other.
pub fn login<'a>(
    paths: &Paths,
    keyring: &'a dyn Keyring,
    client: Client,
    pasted: &str,
) -> Result<LoggedIn<'a>, AccountError> {
    let code = wineo_gog::parse_paste(pasted).map_err(AccountError::Paste)?;
    let requested = Instant::now();
    let tokens = client
        .exchange_code(&code)
        .map_err(|error| match error.class() {
            Class::LoggedOut => AccountError::CodeRejected,
            Class::Temporary => AccountError::Temporary(error),
            Class::Malformed | Class::Other => AccountError::Gog(error),
        })?;
    let login = StoredLogin::from_tokens(&tokens);
    let saved = {
        let _lock = CredentialsLock::acquire(paths)?;
        save(paths, keyring, &login)?
    };
    let mut session = Session {
        paths: paths.clone(),
        keyring,
        client,
        login,
        source: match saved {
            Saved::Keyring => KeySource::Keyring,
            Saved::File(_) => KeySource::File,
        },
        access: None,
    };
    session.set_access(&tokens, requested);
    Ok(LoggedIn {
        user_id: tokens.user_id,
        saved,
        session,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedOut {
    /// The account that was logged out, when its login could be read.
    pub user_id: Option<UserId>,
    pub removed: Removed,
    pub cache_removed: bool,
}

/// Deletes the saved login and its account's cached library. The cache goes
/// first, so a failure leaves the login there to try again with.
pub fn logout(paths: &Paths, keyring: &dyn Keyring) -> Result<LoggedOut, AccountError> {
    let _lock = CredentialsLock::acquire(paths)?;
    let user_id = match saved_login(paths, keyring) {
        Ok(login) => login.map(|login| login.user_id),
        // Unreadable logins are removed all the same.
        Err(AccountError::Unreadable) => None,
        Err(error) => return Err(error),
    };
    let cache_removed = match user_id {
        Some(user_id) => owned::remove_cache(paths, user_id)?,
        None => false,
    };
    let removed = key_store::remove_key(&KEY_SPEC, paths, keyring)?;
    Ok(LoggedOut {
        user_id,
        removed,
        cache_removed,
    })
}

struct Access {
    token: Secret,
    renew_at: Instant,
}

/// A logged-in account, renewing its access token as needed. Every renewal
/// saves the refresh token GOG hands back.
pub struct Session<'a> {
    paths: Paths,
    keyring: &'a dyn Keyring,
    client: Client,
    login: StoredLogin,
    source: KeySource,
    access: Option<Access>,
}

impl fmt::Debug for Session<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Session")
            .field("user_id", &self.login.user_id)
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl<'a> Session<'a> {
    /// The saved login, or None when logged out. Nothing is asked of GOG
    /// until an access token is needed.
    pub fn open(
        paths: &Paths,
        keyring: &'a dyn Keyring,
        client: Client,
    ) -> Result<Option<Self>, AccountError> {
        Ok(stored_login(paths, keyring)?.map(|(login, source)| Self {
            paths: paths.clone(),
            keyring,
            client,
            login,
            source,
            access: None,
        }))
    }

    pub fn user_id(&self) -> UserId {
        self.login.user_id
    }

    pub fn source(&self) -> KeySource {
        self.source
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// A current access token, renewing it first when it is within a minute
    /// of expiring.
    pub fn access_token(&mut self) -> Result<Secret, AccountError> {
        if let Some(access) = &self.access
            && Instant::now() < access.renew_at
        {
            return Ok(access.token.clone());
        }
        self.renew()?;
        Ok(self
            .access
            .as_ref()
            .expect("a renewal sets the access token")
            .token
            .clone())
    }

    /// Runs an authenticated request. When GOG refuses the access token
    /// anyway, it is renewed whatever its lifetime and the request is tried
    /// once more.
    pub fn call<T>(
        &mut self,
        request: impl Fn(&Client, &str) -> Result<T, wineo_gog::Error>,
    ) -> Result<T, CallError> {
        let token = self.access_token().map_err(CallError::Account)?;
        match request(&self.client, token.expose()) {
            Err(error) if error.kind() == ErrorKind::Unauthorized => {
                self.renew().map_err(CallError::Account)?;
                let token = self.access_token().map_err(CallError::Account)?;
                request(&self.client, token.expose()).map_err(CallError::Request)
            }
            result => result.map_err(CallError::Request),
        }
    }

    fn set_access(&mut self, tokens: &Tokens, requested: Instant) {
        let lifetime = Duration::from_secs(tokens.expires_in).saturating_sub(RENEW_EARLY);
        self.access = Some(Access {
            token: tokens.access_token.clone(),
            renew_at: requested + lifetime,
        });
    }

    fn renew(&mut self) -> Result<(), AccountError> {
        self.access = None;
        for _ in 0..RENEW_TRIES {
            let started = self.login.clone();
            let requested = Instant::now();
            let result = self.client.refresh(&started.refresh_token);
            let _lock = CredentialsLock::acquire(&self.paths)?;
            let current = saved_login(&self.paths, self.keyring)?;
            if current.as_ref() == Some(&started) {
                return match result {
                    Ok(tokens) => {
                        let renewed = StoredLogin::from_tokens(&tokens);
                        self.source = match save(&self.paths, self.keyring, &renewed)? {
                            Saved::Keyring => KeySource::Keyring,
                            Saved::File(_) => KeySource::File,
                        };
                        self.login = renewed;
                        self.set_access(&tokens, requested);
                        Ok(())
                    }
                    Err(error) => {
                        let error = renewal_error(error);
                        if matches!(error, AccountError::LoggedOut) {
                            key_store::remove_key(&KEY_SPEC, &self.paths, self.keyring)?;
                        }
                        Err(error)
                    }
                };
            }
            // Another login, logout, or renewal got there first, so this
            // answer is stale whatever it was.
            match current {
                None => return Err(AccountError::LoggedOut),
                Some(newer) => self.login = newer,
            }
        }
        Err(AccountError::Changed)
    }
}

#[cfg(test)]
mod tests;
