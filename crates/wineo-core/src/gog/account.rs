//! The GOG login: one secret, `{refresh_token, user_id}` plus the account's
//! public name when it is known, kept like the API keys (keyring first, then
//! a private file). The access token only ever lives in memory.
//!
//! Login, logout, and renewal all take one host-wide lock. A renewal sends
//! its request without the lock, then saves only if the stored login is
//! still the one it started from, so a stale renewal can't overwrite a newer
//! login or bring one back after a logout.
//!
//! Every change to the saved login under the lock also bumps a generation
//! counter. A session is bound to one account for its whole life: before
//! each authenticated request it compares the counter, and when it moved it
//! reads the login again, carrying on only if it is still the same account.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

use rustix::fs::{FlockOperation, flock};
use serde::{Deserialize, Serialize};
use wineo_gog::{Class, Client, ErrorKind, PasteError, Secret, Tokens, UserId};

use super::owned;
use crate::error::IoContext;
use crate::files::write_atomic;
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
    /// GOG's public name for the account. Logins saved before names were
    /// looked up don't have it, and a lookup that failed leaves it out. It
    /// isn't written when absent, so those logins read back byte for byte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

impl fmt::Debug for StoredLogin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredLogin")
            .field("refresh_token", &"<redacted>")
            .field("user_id", &self.user_id)
            .field("username", &self.username)
            .finish()
    }
}

impl StoredLogin {
    fn from_tokens(tokens: &Tokens) -> Self {
        Self {
            refresh_token: tokens.refresh_token.expose().to_owned(),
            user_id: tokens.user_id,
            username: None,
        }
    }

    /// Whether `other` holds the same grant. The name doesn't count: another
    /// process filling it in leaves a renewal that started from this refresh
    /// token just as valid, so it isn't taken for a newer login.
    fn same_grant(&self, other: &Self) -> bool {
        self.refresh_token == other.refresh_token && self.user_id == other.user_id
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
    /// Another account logged in. A session never switches accounts, so
    /// nothing it fetched is kept for either one.
    #[error("Another GOG account logged in while this was running. Try again.")]
    Switched,
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

impl From<wineo_gog::Error> for CallError {
    fn from(error: wineo_gog::Error) -> Self {
        Self::Request(error)
    }
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

impl CredentialsLock {
    /// The lock if its file exists, else None. No file means nothing has
    /// ever been saved, so there is nothing to wait for, and a read-only
    /// caller doesn't create one.
    fn acquire_existing(paths: &Paths) -> crate::Result<Option<Self>> {
        let path = paths.gog_credentials_lock();
        let file = match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).context(|| format!("failed to open {}", path.display()));
            }
        };
        flock(&file, FlockOperation::LockExclusive)
            .map_err(std::io::Error::from)
            .context(|| format!("failed to lock {}", path.display()))?;
        Ok(Some(Self { file }))
    }
}

impl Drop for CredentialsLock {
    fn drop(&mut self) {
        // A forked child must not extend this holder's lifetime.
        let _ = flock(&self.file, FlockOperation::Unlock);
    }
}

/// The current credentials generation. A missing or unreadable counter is
/// zero, which the next bump moves past.
fn read_generation(paths: &Paths) -> crate::Result<u64> {
    let path = paths.gog_credentials_generation();
    match fs::read_to_string(&path) {
        Ok(text) => Ok(text.trim().parse().unwrap_or(0)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error).context(|| format!("failed to read {}", path.display())),
    }
}

/// Moves the generation on. Only called holding the credentials lock, and
/// always before the credentials change: a crash or failure after the bump
/// only makes sessions read the login again, while one before a bump that
/// never happened would leave them trusting a login that is gone.
fn bump_generation(paths: &Paths, _lock: &CredentialsLock) -> crate::Result<u64> {
    let next = read_generation(paths)?.wrapping_add(1);
    write_atomic(&paths.gog_credentials_generation(), format!("{next}\n"))?;
    Ok(next)
}

/// Saves a login as the current one, under the lock.
pub(crate) fn store(
    paths: &Paths,
    keyring: &dyn Keyring,
    login: &StoredLogin,
) -> crate::Result<(Saved, u64)> {
    let lock = CredentialsLock::acquire(paths)?;
    let generation = bump_generation(paths, &lock)?;
    let saved = save(paths, keyring, login)?;
    Ok((saved, generation))
}

fn source_of(saved: &Saved) -> KeySource {
    match saved {
        Saved::Keyring => KeySource::Keyring,
        Saved::File(_) => KeySource::File,
    }
}

#[derive(Debug)]
pub struct LoggedIn<'a> {
    pub user_id: UserId,
    /// None when GOG didn't say, which doesn't fail the login.
    pub username: Option<String>,
    pub saved: Saved,
    /// Ready to use, with the access token from the login.
    pub session: Session<'a>,
}

/// Logs in with what the user pasted after GOG's login page: trades the code
/// for tokens, looks up the account's name, then saves the login, replacing
/// any other. A name that can't be looked up is left out.
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
    let mut login = StoredLogin::from_tokens(&tokens);
    // Before the lock, since GOG may be slow, and ignoring failure because
    // the name is only for show.
    login.username = client.username(tokens.user_id).ok();
    let (saved, generation) = store(paths, keyring, &login)?;
    let mut session = Session {
        paths: paths.clone(),
        keyring,
        client,
        login,
        source: source_of(&saved),
        generation,
        access: None,
        name_looked_up: true,
    };
    session.set_access(&tokens, requested);
    Ok(LoggedIn {
        user_id: tokens.user_id,
        username: session.login.username.clone(),
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
/// first, so a failure leaves the login there to try again with. Sessions
/// publish their caches under the same lock, so none can bring it back.
pub fn logout(paths: &Paths, keyring: &dyn Keyring) -> Result<LoggedOut, AccountError> {
    let lock = CredentialsLock::acquire(paths)?;
    let user_id = match saved_login(paths, keyring) {
        Ok(login) => login.map(|login| login.user_id),
        // Unreadable logins are removed all the same.
        Err(AccountError::Unreadable) => None,
        Err(error) => return Err(error),
    };
    bump_generation(paths, &lock)?;
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
/// saves the refresh token GOG hands back. A session stays with the account
/// it opened with: a newer refresh token for that account is adopted, but
/// another account or a logout ends it.
pub struct Session<'a> {
    paths: Paths,
    keyring: &'a dyn Keyring,
    client: Client,
    login: StoredLogin,
    source: KeySource,
    /// The credentials generation `login` was read or saved at.
    generation: u64,
    access: Option<Access>,
    /// Whether the name was already asked for in this session, so a login
    /// without one costs a single lookup rather than one per renewal.
    name_looked_up: bool,
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
        // Changes bump the generation before they save, so reading the two
        // without the lock could pair a new generation with the old login.
        // Opening only reads, so it doesn't create the lock: a dry run that
        // never renews leaves no file behind.
        let _lock = CredentialsLock::acquire_existing(paths)?;
        let generation = read_generation(paths)?;
        Ok(stored_login(paths, keyring)?.map(|(login, source)| Self {
            paths: paths.clone(),
            keyring,
            client,
            login,
            source,
            generation,
            access: None,
            name_looked_up: false,
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

    /// Whether the saved login is still this session's account. Cheap when
    /// nothing changed: one small file read.
    pub fn check(&mut self) -> Result<(), AccountError> {
        if read_generation(&self.paths)? == self.generation {
            return Ok(());
        }
        let _lock = CredentialsLock::acquire(&self.paths)?;
        self.resync()
    }

    /// Reads the saved login again. Only called holding the lock.
    fn resync(&mut self) -> Result<(), AccountError> {
        let generation = read_generation(&self.paths)?;
        match saved_login(&self.paths, self.keyring)? {
            Some(login) if login.user_id == self.login.user_id => {
                self.login = login;
                self.generation = generation;
                Ok(())
            }
            other => {
                self.access = None;
                Err(if other.is_some() {
                    AccountError::Switched
                } else {
                    AccountError::LoggedOut
                })
            }
        }
    }

    /// Runs `write` holding the credentials lock, only while the saved login
    /// is still this session's account. Whatever it writes for the account
    /// can't land after a logout or a switch.
    pub fn while_current<T>(&mut self, write: impl FnOnce() -> T) -> Result<T, AccountError> {
        let _lock = CredentialsLock::acquire(&self.paths)?;
        self.resync()?;
        Ok(write())
    }

    /// A current access token, renewing it first when it is within a minute
    /// of expiring.
    pub fn access_token(&mut self) -> Result<Secret, AccountError> {
        self.check()?;
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
    ///
    /// The login is checked before each request and again after each
    /// answer. Holding the credentials lock across the network would stall
    /// every login and logout behind GOG, so a logout or switch can happen
    /// while a request is in flight. That request is left to finish, but its
    /// answer is thrown away: nothing fetched for an account that has left
    /// is ever returned.
    pub fn call<T>(
        &mut self,
        request: impl Fn(&Client, &str) -> Result<T, wineo_gog::Error>,
    ) -> Result<T, CallError> {
        let token = self.access_token().map_err(CallError::Account)?;
        let mut result = request(&self.client, token.expose());
        self.check().map_err(CallError::Account)?;
        if let Err(error) = &result
            && error.kind() == ErrorKind::Unauthorized
        {
            self.renew().map_err(CallError::Account)?;
            let token = self.access_token().map_err(CallError::Account)?;
            result = request(&self.client, token.expose());
            self.check().map_err(CallError::Account)?;
        }
        result.map_err(CallError::Request)
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
        let mut fetched = None;
        for _ in 0..RENEW_TRIES {
            let started = self.login.clone();
            let requested = Instant::now();
            let result = self.client.refresh(&started.refresh_token);
            // A login saved before names were kept gets its name now, while
            // GOG is being talked to anyway. Best effort and outside the
            // lock: a failure changes nothing about the renewal.
            if started.username.is_none()
                && !self.name_looked_up
                && result
                    .as_ref()
                    .is_ok_and(|tokens| tokens.user_id == started.user_id)
            {
                self.name_looked_up = true;
                fetched = self.client.username(started.user_id).ok();
            }
            let lock = CredentialsLock::acquire(&self.paths)?;
            let current = saved_login(&self.paths, self.keyring)?;
            if let Some(current) = current.filter(|current| current.same_grant(&started)) {
                return match result {
                    // GOG never should, but tokens for another account
                    // mustn't be saved over this one's.
                    Ok(tokens) if tokens.user_id != started.user_id => Err(AccountError::Switched),
                    Ok(tokens) => {
                        let mut renewed = StoredLogin::from_tokens(&tokens);
                        // The saved name wins, since another process may
                        // have filled it in since this one started.
                        renewed.username = current.username.or(fetched);
                        let generation = bump_generation(&self.paths, &lock)?;
                        self.source = source_of(&save(&self.paths, self.keyring, &renewed)?);
                        self.generation = generation;
                        self.login = renewed;
                        self.set_access(&tokens, requested);
                        Ok(())
                    }
                    Err(error) => {
                        let error = renewal_error(error);
                        if matches!(error, AccountError::LoggedOut) {
                            bump_generation(&self.paths, &lock)?;
                            key_store::remove_key(&KEY_SPEC, &self.paths, self.keyring)?;
                        }
                        Err(error)
                    }
                };
            }
            // Another login, logout, or renewal got there first, so this
            // answer is stale whatever it was. The same account's newer
            // login is tried; anything else ends the session.
            self.resync()?;
        }
        Err(AccountError::Changed)
    }
}

#[cfg(test)]
mod tests;
