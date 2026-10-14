//! What GOG adds to a game once it is migrated: its playtime and artwork.
//! This runs after the migration has succeeded, so nothing here fails it.

use wineo_core::art::Kind;
use wineo_core::config::GlobalConfig;
use wineo_core::download::{Https, Lazy};
use wineo_core::gog::account::Session;
use wineo_core::gog::extras::{self, Request};
use wineo_core::paths::Paths;
use wineo_core::steamgriddb;

use crate::commands::art::{self, KeyCheck};
use crate::commands::gog_support::{Account, warn};

/// What a run of imports shares: the login session, GOG's artwork client and
/// the HTTPS client, each built when the first game needs it. A bulk migrate
/// would otherwise build them for every game, and renew the access token for
/// each one. The session still renews when its token nears expiry, so a long
/// batch renews as often as the token's lifetime says.
pub(super) struct Batch<'a> {
    account: &'a Account,
    /// The first game's attempt to open the login. A failure is kept as the
    /// text each game warns with.
    session: Option<Result<Option<Session<'a>>, String>>,
    gog: Option<Result<wineo_core::gog::api::Client, wineo_core::gog::api::Error>>,
    https: Lazy<Https>,
}

impl<'a> Batch<'a> {
    pub(super) fn new(account: &'a Account) -> Self {
        Self {
            account,
            session: None,
            gog: None,
            https: Lazy::new(),
        }
    }

    /// Imports GOG's playtime and art for `environment`, says what it did, and
    /// returns whether an icon was added, which desktop entries need to know.
    pub(super) fn import(
        &mut self,
        paths: &Paths,
        global: &GlobalConfig,
        environment: &str,
        gog_id: u64,
        title: &str,
    ) -> bool {
        // Playtime needs the login and art doesn't, so a login that can't be
        // read only costs the playtime.
        let account = self.account;
        // A login that was revoked, cleared, or switched since the last game
        // is read fresh, as opening a session per game used to.
        if let Some(Ok(Some(session))) = &mut self.session
            && session.check().is_err()
        {
            self.session = None;
        }
        let session = self
            .session
            .get_or_insert_with(|| account.open(paths).map_err(|error| format!("{error:#}")));
        let session = match session {
            Ok(session) => session.as_mut(),
            Err(error) => {
                warn(format!("Skipped GOG playtime: {error}"));
                None
            }
        };
        let transport = match self.https.https() {
            Ok(transport) => transport,
            Err(error) => {
                warn(format!("Skipped GOG playtime and art: {error:#}"));
                return false;
            }
        };
        let key = match art::check_key(paths) {
            KeyCheck::Found(key) => Some(key.value),
            KeyCheck::Missing => None,
            failed @ KeyCheck::Failed(_) => {
                failed.print_skip();
                None
            }
        };
        let language = steamgriddb::language(|var| std::env::var_os(var));
        let kinds = global.art.fetch.kinds();
        let request = Request {
            name: environment,
            gog_id,
            title,
            kinds: &kinds,
            transport,
            steamgriddb_key: key.as_deref(),
            language: &language,
        };
        let gog = self
            .gog
            .get_or_insert_with(wineo_core::gog::client_from_env)
            .as_ref();
        // Ctrl+C here keeps its usual meaning; the migration is already done and
        // this part is short, so it isn't canceled separately.
        let token = wineo_core::cancel::Token::new();
        match extras::import_with(paths, session, &request, gog, &token) {
            Ok(report) => {
                for line in report.lines() {
                    println!("{line}");
                }
                for note in &report.notes {
                    warn(note);
                }
                report.art.iter().any(|filled| filled.kind == Kind::Icon)
            }
            Err(error) => {
                warn(format!("Couldn't import GOG playtime and art: {error:#}"));
                false
            }
        }
    }
}

/// Imports GOG's playtime and art for one game.
pub(super) fn import(
    paths: &Paths,
    global: &GlobalConfig,
    environment: &str,
    gog_id: u64,
    title: &str,
) -> bool {
    let account = Account::new();
    Batch::new(&account).import(paths, global, environment, gog_id, title)
}
