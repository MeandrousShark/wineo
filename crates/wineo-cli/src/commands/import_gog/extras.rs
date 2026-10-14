//! What GOG adds to a game once it is migrated: its playtime and artwork.
//! This runs after the migration has succeeded, so nothing here fails it.

use wineo_core::art::{Kind, Source};
use wineo_core::config::GlobalConfig;
use wineo_core::download::{Https, Lazy};
use wineo_core::gog::account::Session;
use wineo_core::gog::extras::{self, Report, Request};
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
                for line in lines(&report) {
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

/// What the report says happened, in a line or two.
fn lines(report: &Report) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(playtime) = report.playtime.filter(|playtime| playtime.raised()) {
        lines.push(format!(
            "Imported {} of GOG playtime.",
            duration(playtime.gog_seconds)
        ));
    }
    let gog: fn(&Source) -> bool = |source| matches!(source, Source::Gog(_));
    let grid: fn(&Source) -> bool = |source| matches!(source, Source::SteamGridDb(_));
    for (name, wanted) in [("GOG", gog), ("SteamGridDB", grid)] {
        let kinds: Vec<&str> = [Kind::Grid, Kind::Hero, Kind::Logo, Kind::Icon]
            .into_iter()
            .filter(|kind| {
                report
                    .art
                    .iter()
                    .any(|filled| filled.kind == *kind && wanted(&filled.source))
            })
            .map(kind_name)
            .collect();
        if !kinds.is_empty() {
            lines.push(format!("Added {name} art: {}.", kinds.join(", ")));
        }
    }
    lines
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Grid => "poster",
        other => other.as_str(),
    }
}

/// Whole hours, or minutes for under an hour.
fn duration(seconds: i64) -> String {
    if seconds >= 3600 {
        format!("{} h", (seconds + 1800) / 3600)
    } else {
        format!("{} min", seconds / 60)
    }
}

#[cfg(test)]
mod tests {
    use wineo_core::gog::extras::{Filled, Playtime};

    use super::*;

    fn filled(kind: Kind, source: Source) -> Filled {
        Filled { kind, source }
    }

    #[test]
    fn playtime_and_art_each_get_a_short_line() {
        let report = Report {
            playtime: Some(Playtime {
                gog_seconds: 41 * 3600 + 600,
                before_seconds: 0,
                after_seconds: 41 * 3600 + 600,
            }),
            art: vec![
                filled(Kind::Icon, Source::Gog(1)),
                filled(Kind::Grid, Source::Gog(1)),
                filled(Kind::Hero, Source::Gog(1)),
                filled(Kind::Logo, Source::SteamGridDb(9)),
            ],
            notes: Vec::new(),
        };
        assert_eq!(
            lines(&report),
            [
                "Imported 41 h of GOG playtime.",
                "Added GOG art: poster, hero, icon.",
                "Added SteamGridDB art: logo.",
            ]
        );
    }

    #[test]
    fn playtime_that_didnt_rise_and_no_art_say_nothing() {
        let report = Report {
            playtime: Some(Playtime {
                gog_seconds: 600,
                before_seconds: 3600,
                after_seconds: 3600,
            }),
            ..Report::default()
        };
        assert!(lines(&report).is_empty());
    }

    #[test]
    fn short_playtime_is_in_minutes() {
        assert_eq!(duration(25 * 60 + 30), "25 min");
        assert_eq!(duration(3600), "1 h");
    }
}
