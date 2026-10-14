//! Playtime tracking: an append-only session log per environment.
//!
//! `$XDG_DATA_HOME/wineo/stats/<env>.jsonl`, or `<folder>/stats/<env>.jsonl`
//! in a portable folder (see `Paths::stats_log`). One JSON object per line: a
//! session, or an `imported` line Heroic playtime is carried over as. The log
//! is strictly append-only, including a re-import: reading keeps the last
//! `imported` line it sees. Totals are computed on read, so nothing has to
//! stay in sync.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use jiff::civil::Date;
use jiff::{Span, Timestamp};
use serde::{Deserialize, Serialize};

use crate::environment::{self, Environment};
use crate::error::{IoContext, Result};
use crate::heroic::HeroicSource;
use crate::paths::Paths;
use crate::runtime::{Runtime, State};
use crate::time::{LocalZone, Zone};
use crate::{config::GlobalConfig, steam};

/// How a session came to an end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum End {
    Exited,
    Failed,
    Stopped,
    /// The supervisor died without recording how the run ended. Its session
    /// ends at its last heartbeat rather than whenever it was cleaned up.
    Lost,
}

/// One target run, from `started_at` to `ended_at`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub target: String,
    pub started_at: Timestamp,
    pub ended_at: Timestamp,
    pub seconds: i64,
    pub end: End,
}

impl Session {
    pub fn new(target: String, started_at: Timestamp, ended_at: Timestamp, end: End) -> Self {
        let seconds = ended_at.duration_since(started_at).as_secs().max(0);
        Self {
            target,
            started_at,
            ended_at,
            seconds,
            end,
        }
    }
}

/// A starting total carried over from Heroic. It counts toward the total but
/// not sessions or averages. Importing again appends another `imported` line
/// with Heroic's current total; reading a log keeps only the last one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Imported {
    pub seconds: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_played: Option<Timestamp>,
}

/// The `imported` line's shape on disk: `{"imported": {...}}`, which is how a
/// session line (always missing that key) is told apart from it.
#[derive(Debug, Serialize, Deserialize)]
struct ImportedLine {
    imported: Imported,
}

/// A line neither a session nor an `imported` line parses as. Distinguished
/// structurally rather than by a tag, since unknown fields are ignored and a
/// session line never carries an `imported` key.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Line {
    Session(Session),
    Imported(ImportedLine),
}

/// Appends one line to the log at `path`, creating it and its parent
/// directory if needed. O_APPEND makes this crash-safe without a
/// read-modify-write or a lock: a line is never lost or torn by a concurrent
/// append.
fn append_line(path: &Path, line: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .context(|| format!("failed to open {}", path.display()))?;
    let mut text = serde_json::to_string(line).expect("a log line always serializes");
    text.push('\n');
    file.write_all(text.as_bytes())
        .context(|| format!("failed to write {}", path.display()))
}

/// Appends one session line to the log at `path`.
pub fn append(path: &Path, session: &Session) -> Result<()> {
    append_line(path, session)
}

/// Appends `name`'s Heroic playtime as a new `imported` line. The log is
/// append-only even here: a re-import doesn't rewrite the earlier line, it
/// adds a new one, and reading the log uses whichever comes last.
pub fn set_imported(paths: &Paths, name: &str, imported: Imported) -> Result<()> {
    append_line(&paths.stats_log(name), &ImportedLine { imported })
}

/// Copies `name`'s play log from one root to another, for `portable copy`,
/// which carries it like art since it's the same game moving. Returns where
/// it went, or None if there was no log to copy.
pub(crate) fn copy_log(from: &Paths, to: &Paths, name: &str) -> Result<Option<PathBuf>> {
    let source = from.stats_log(name);
    let bytes = match fs::read(&source) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).context(|| format!("failed to read {}", source.display())),
    };
    let destination = to.stats_log(name);
    publish_new(&destination, &bytes)
        .context(|| format!("failed to write {}", destination.display()))?;
    Ok(Some(destination))
}

/// Writes a file that must not replace one already there: a synced temp
/// file is linked into place, which fails if the name is taken.
fn publish_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut staging = tempfile::NamedTempFile::new_in(parent)?;
    staging.write_all(bytes)?;
    staging.as_file().sync_all()?;
    fs::hard_link(staging.path(), path)?;
    drop(staging);
    fs::File::open(parent)?.sync_all()
}

/// Everything read back from the log: its sessions, its `imported` line if
/// any, and lines that could not be read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Log {
    pub sessions: Vec<Session>,
    pub imported: Option<Imported>,
    /// One message per line that was skipped, so a torn last line never loses
    /// the rest of the log.
    pub warnings: Vec<String>,
}

/// Reads and parses the log at `path`. A missing file reads as empty.
pub fn read(path: &Path) -> Result<Log> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Log::default()),
        Err(err) => return Err(err).context(|| format!("failed to read {}", path.display())),
    };
    let mut log = Log::default();
    for (number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Line>(line) {
            Ok(Line::Session(session)) => log.sessions.push(session),
            Ok(Line::Imported(ImportedLine { imported })) => log.imported = Some(imported),
            Err(err) => log.warnings.push(format!(
                "skipping unreadable line {} in {}: {err}",
                number + 1,
                path.display()
            )),
        }
    }
    Ok(log)
}

/// A session that hasn't ended yet, read from the live `RunState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub target: String,
    pub started_at: Timestamp,
}

/// One target's totals within a `Summary`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetSummary {
    pub target: String,
    pub seconds: i64,
    pub sessions: usize,
}

/// The longest session in a `Summary`, with when it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Longest {
    pub seconds: i64,
    pub started_at: Timestamp,
}

/// Everything `wineo stats` shows for one environment, computed on read.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    /// This environment's total from Wineo sessions and imports, or Steam.
    pub seconds: i64,
    /// Logged sessions plus an in-progress one. Excludes the imported total.
    pub sessions: usize,
    pub average_seconds: Option<i64>,
    pub longest: Option<Longest>,
    pub first_played: Option<Timestamp>,
    pub last_played: Option<Timestamp>,
    pub this_week_seconds: i64,
    pub this_month_seconds: i64,
    pub running: bool,
    pub imported_seconds: i64,
    /// Which source supplies the playtime total.
    pub source: Source,
    /// Steam has no readable total for this game.
    pub playtime_unknown: bool,
    /// Sorted by time played, most first.
    pub targets: Vec<TargetSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Wineo,
    Steam,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wineo => "wineo",
            Self::Steam => "steam",
        }
    }
}

/// One environment whose stats could not be read for the overview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewError {
    pub name: String,
    pub message: String,
}

/// Every readable environment's playtime and the totals for a library view.
#[derive(Debug, Clone)]
pub struct Overview {
    /// Environment name order, including never-played environments.
    pub rows: Vec<(String, Summary)>,
    /// Tags for included environments, populated only when requested.
    pub tags: BTreeMap<String, Vec<String>>,
    /// Indices into `rows`, sorted by time played and then name.
    pub played: Vec<usize>,
    pub total_seconds: i64,
    pub never_played: usize,
    /// Known Steam totals included in the overview.
    pub steam_games: usize,
    /// Steam games whose playtime could not be read.
    pub unknown_steam_games: usize,
    pub warnings: Vec<String>,
    pub errors: Vec<OverviewError>,
    pub now: Timestamp,
    pub tz: LocalZone,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct OverviewOptions<'a> {
    /// Every given tag must be present.
    pub tags: Option<&'a [String]>,
    /// Load tags even without a filter, for `by_tag`.
    pub load_tags: bool,
}

/// Collects every environment's playtime without stopping for one bad log.
pub fn overview(paths: &Paths) -> Result<Overview> {
    overview_with_options(paths, OverviewOptions::default())
}

/// Loads configs only when filtering or collecting tags for a breakdown.
pub fn overview_with_options(paths: &Paths, options: OverviewOptions<'_>) -> Result<Overview> {
    let now = Timestamp::now();
    let tz = LocalZone::new();
    let mut rows = Vec::new();
    let mut tags = BTreeMap::new();
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    // One reading of Steam's files for every Steam game in the overview.
    let steam = SteamPlaytimes::new(paths);
    for name in environment::names(paths)? {
        let (environment_tags, loaded) = if options.tags.is_some() || options.load_tags {
            match Environment::load(paths, &name) {
                Ok(environment) => {
                    let tags = &environment.config.tags;
                    if options
                        .tags
                        .is_some_and(|filter| !filter.iter().all(|tag| tags.contains(tag)))
                    {
                        continue;
                    }
                    (Some(tags.clone()), Some(environment.config))
                }
                Err(err) => {
                    errors.push(OverviewError {
                        name,
                        message: format!("{:#}", anyhow::Error::from(err)),
                    });
                    continue;
                }
            }
        } else {
            (None, None)
        };
        // A config the tags already loaded isn't read and parsed again.
        let config = loaded.or_else(|| lenient_config(paths, &name));
        match summary_of(paths, &name, config.as_ref(), &steam, now, &tz) {
            Ok((summary, _, warns)) => {
                warnings.extend(warns);
                if let Some(environment_tags) = environment_tags {
                    tags.insert(name.clone(), environment_tags);
                }
                rows.push((name, summary));
            }
            Err(err) => errors.push(OverviewError {
                name,
                message: format!("{:#}", anyhow::Error::from(err)),
            }),
        }
    }
    let mut played: Vec<usize> = (0..rows.len())
        .filter(|&index| rows[index].1.seconds > 0)
        .collect();
    played.sort_by(|&a, &b| {
        rows[b]
            .1
            .seconds
            .cmp(&rows[a].1.seconds)
            .then_with(|| rows[a].0.cmp(&rows[b].0))
    });
    let steam_games = rows
        .iter()
        .filter(|(_, summary)| summary.source == Source::Steam && !summary.playtime_unknown)
        .count();
    let unknown_steam_games = rows
        .iter()
        .filter(|(_, summary)| summary.source == Source::Steam && summary.playtime_unknown)
        .count();
    let never_played = rows.len() - played.len() - unknown_steam_games;
    let total_seconds = rows
        .iter()
        .filter(|(_, summary)| !summary.playtime_unknown)
        .map(|(_, summary)| summary.seconds)
        .sum();
    Ok(Overview {
        rows,
        tags,
        played,
        total_seconds,
        never_played,
        steam_games,
        unknown_steam_games,
        warnings,
        errors,
        now,
        tz,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagTotals {
    pub seconds: i64,
    pub sessions: usize,
    pub environments: usize,
    pub played_environments: usize,
    pub last_played: Option<Timestamp>,
}

impl TagTotals {
    fn add(&mut self, summary: &Summary) {
        self.seconds += summary.seconds;
        self.sessions += summary.sessions;
        self.environments += 1;
        self.played_environments += usize::from(summary.seconds > 0);
        self.last_played = self.last_played.max(summary.last_played);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagBreakdown {
    /// Sorted by seconds descending, then tag name.
    pub tags: Vec<(String, TagTotals)>,
    pub untagged: Option<TagTotals>,
}

/// Groups included environments whose tags were loaded by `overview_with_options`.
pub fn by_tag(overview: &Overview) -> TagBreakdown {
    let mut tags = BTreeMap::<String, TagTotals>::new();
    let mut untagged = None;
    for (name, summary) in &overview.rows {
        let Some(environment_tags) = overview.tags.get(name) else {
            continue;
        };
        if environment_tags.is_empty() {
            untagged.get_or_insert_with(TagTotals::default).add(summary);
        }
        for tag in environment_tags {
            tags.entry(tag.clone()).or_default().add(summary);
        }
    }
    let mut tags: Vec<_> = tags.into_iter().collect();
    tags.sort_by(|a, b| b.1.seconds.cmp(&a.1.seconds).then_with(|| a.0.cmp(&b.0)));
    TagBreakdown { tags, untagged }
}

/// The duration of the last played session, including a run still in progress.
pub fn last_session_seconds(log: &Log, summary: &Summary, now: Timestamp) -> Option<i64> {
    if summary.running {
        return summary
            .last_played
            .map(|started_at| now.duration_since(started_at).as_secs().max(0));
    }
    let last_played = summary.last_played?;
    log.sessions
        .iter()
        .find(|session| session.started_at == last_played)
        .map(|session| session.seconds)
}

/// One result from refreshing Heroic playtime across all environments.
#[derive(Debug)]
pub enum HeroicImportResult {
    Imported { name: String, seconds: i64 },
    EnvironmentError { name: String, error: crate::Error },
    PlaytimeError { name: String, error: crate::Error },
    ImportError { name: String, error: crate::Error },
}

/// Refreshes every linked environment, preserving the discovered source order.
pub fn import_heroic_all(
    paths: &Paths,
    sources: &[HeroicSource],
) -> Result<Vec<HeroicImportResult>> {
    let mut results = Vec::new();
    for name in environment::names(paths)? {
        let environment = match Environment::load(paths, &name) {
            Ok(environment) => environment,
            Err(error) => {
                results.push(HeroicImportResult::EnvironmentError { name, error });
                continue;
            }
        };
        let Some(heroic_id) = &environment.config.heroic_id else {
            continue;
        };
        let mut playtime = None;
        for source in sources {
            match source.playtime(heroic_id) {
                Ok(Some(found)) => {
                    playtime = Some(found);
                    break;
                }
                Ok(None) => {}
                Err(error) => results.push(HeroicImportResult::PlaytimeError {
                    name: name.clone(),
                    error,
                }),
            }
        }
        let Some(playtime) = playtime else {
            continue;
        };
        let imported = Imported {
            seconds: playtime.seconds,
            last_played: playtime.last_played,
        };
        if let Err(error) = set_imported(paths, &name, imported) {
            results.push(HeroicImportResult::ImportError { name, error });
            continue;
        }
        results.push(HeroicImportResult::Imported {
            name,
            seconds: playtime.seconds,
        });
    }
    Ok(results)
}

/// Builds the summary a `Log` and an optional in-progress session add up to,
/// as of `now`. `this_week` and `this_month` bucket by each session's local
/// start date in `tz`.
pub fn summarize(log: &Log, running: Option<&Running>, now: Timestamp, tz: &dyn Zone) -> Summary {
    let live = running.map(|running| {
        Session::new(
            running.target.clone(),
            running.started_at,
            now,
            End::Stopped, // never recorded; the live session has no `end` of its own
        )
    });
    let sessions: Vec<&Session> = log.sessions.iter().chain(live.as_ref()).collect();

    let seconds: i64 = sessions.iter().map(|session| session.seconds).sum();
    let imported_seconds = log.imported.map(|imported| imported.seconds).unwrap_or(0);
    let average_seconds = (!sessions.is_empty()).then(|| seconds / sessions.len() as i64);
    let longest = sessions
        .iter()
        .max_by_key(|session| session.seconds)
        .map(|session| Longest {
            seconds: session.seconds,
            started_at: session.started_at,
        });
    let first_played = sessions.iter().map(|session| session.started_at).min();
    let last_played = sessions
        .iter()
        .map(|session| session.started_at)
        .chain(log.imported.and_then(|imported| imported.last_played))
        .max();

    // With no sessions the zone is never consulted, so it is never loaded.
    let (this_week_seconds, this_month_seconds) = if sessions.is_empty() {
        (0, 0)
    } else {
        let week_start = start_of_week(now, tz);
        let month_start = start_of_month(now, tz);
        let since = |start: Date| {
            sessions
                .iter()
                .filter(|session| local_date(session.started_at, tz) >= start)
                .map(|session| session.seconds)
                .sum()
        };
        (since(week_start), since(month_start))
    };

    let mut by_target: Vec<TargetSummary> = Vec::new();
    for session in &sessions {
        match by_target
            .iter_mut()
            .find(|target| target.target == session.target)
        {
            Some(target) => {
                target.seconds += session.seconds;
                target.sessions += 1;
            }
            None => by_target.push(TargetSummary {
                target: session.target.clone(),
                seconds: session.seconds,
                sessions: 1,
            }),
        }
    }
    by_target.sort_by(|a, b| {
        b.seconds
            .cmp(&a.seconds)
            .then_with(|| a.target.cmp(&b.target))
    });

    Summary {
        seconds: seconds + imported_seconds,
        sessions: sessions.len(),
        average_seconds,
        longest,
        first_played,
        last_played,
        this_week_seconds,
        this_month_seconds,
        running: running.is_some(),
        imported_seconds,
        source: Source::Wineo,
        playtime_unknown: false,
        targets: by_target,
    }
}

/// `name`'s run right now, if it counts as a live session. A lost run does
/// not: it isn't over, but nobody can say it's still going either.
pub fn running_session(paths: &Paths, name: &str) -> Result<Option<Running>> {
    let status = Runtime::new(paths, name).status()?;
    Ok((status.state == State::Running && !status.lost)
        .then_some(status.run)
        .flatten()
        .map(|run| Running {
            target: run.target,
            started_at: run.started_at,
        }))
}

/// Reads `name`'s playtime and folds in its live run, if any, as of `now`.
/// Steam-backed environments do not read their Wineo session log.
pub fn summary(
    paths: &Paths,
    name: &str,
    now: Timestamp,
    tz: &dyn Zone,
) -> Result<(Summary, Vec<String>)> {
    let (summary, _, warnings) = summary_with_log(paths, name, now, tz)?;
    Ok((summary, warnings))
}

/// Reads a Wineo session log with its summary, returning no log for Steam.
pub fn summary_with_log(
    paths: &Paths,
    name: &str,
    now: Timestamp,
    tz: &dyn Zone,
) -> Result<(Summary, Option<Log>, Vec<String>)> {
    let config = lenient_config(paths, name);
    summary_of(
        paths,
        name,
        config.as_ref(),
        &SteamPlaytimes::new(paths),
        now,
        tz,
    )
}

/// The environment's config as far as it parses. A config that can't be read
/// is treated as a Wineo game, whose log is read all the same.
fn lenient_config(paths: &Paths, name: &str) -> Option<crate::config::EnvironmentConfig> {
    let config_path = paths.environment_config(name);
    #[cfg(test)]
    crate::read_log::note(&config_path);
    let text = fs::read_to_string(&config_path).ok()?;
    crate::config::EnvironmentConfig::parse(&config_path, &text).ok()
}

/// `summary_with_log` for a config already read, with Steam's files shared
/// with the other environments of the same overview.
fn summary_of(
    paths: &Paths,
    name: &str,
    config: Option<&crate::config::EnvironmentConfig>,
    steam: &SteamPlaytimes<'_>,
    now: Timestamp,
    tz: &dyn Zone,
) -> Result<(Summary, Option<Log>, Vec<String>)> {
    if let Some(config) = config
        && let crate::config::Kind::Proxied(proxied) = &config.kind
    {
        return Ok((
            summarize_steam(steam.playtime(&proxied.app_id)),
            None,
            Vec::new(),
        ));
    }
    let log = read(&paths.stats_log(name))?;
    let running = running_session(paths, name)?;
    let warnings = log.warnings.clone();
    Ok((
        summarize(&log, running.as_ref(), now, tz),
        Some(log),
        warnings,
    ))
}

/// Steam's playtime for the games of one overview. The global config, the
/// install, and the account's `localconfig.vdf` are found and read once, on
/// the first Steam game that asks, not again for each one.
struct SteamPlaytimes<'a> {
    paths: &'a Paths,
    /// The reason nothing can be read, or the parsed playtime file.
    playtimes: std::cell::OnceCell<std::result::Result<steam::Playtimes, String>>,
}

impl<'a> SteamPlaytimes<'a> {
    fn new(paths: &'a Paths) -> Self {
        Self {
            paths,
            playtimes: std::cell::OnceCell::new(),
        }
    }

    fn playtime(&self, app_id: &str) -> steam::Playtime {
        let playtimes = self
            .playtimes
            .get_or_init(|| match GlobalConfig::load(self.paths) {
                Ok(global) => steam::active_for(self.paths, &global)
                    .map(|install| steam::Playtimes::load(&install))
                    .ok_or_else(|| "Steam was not found".to_owned()),
                Err(error) => Err(format!("global config can't be read: {error}")),
            });
        match playtimes {
            Ok(playtimes) => playtimes.get(app_id),
            Err(reason) => steam::Playtime::Unknown(reason.clone()),
        }
    }
}

fn summarize_steam(playtime: steam::Playtime) -> Summary {
    let (seconds, last_played, playtime_unknown) = match playtime {
        steam::Playtime::Known {
            seconds,
            last_played,
        } => (
            seconds.min(i64::MAX as u64) as i64,
            last_played
                .and_then(|seconds| i64::try_from(seconds).ok())
                .and_then(|seconds| Timestamp::from_second(seconds).ok()),
            false,
        ),
        steam::Playtime::Unknown(_) => (0, None, true),
    };
    Summary {
        seconds,
        sessions: 0,
        average_seconds: None,
        longest: None,
        first_played: None,
        last_played,
        this_week_seconds: 0,
        this_month_seconds: 0,
        running: false,
        imported_seconds: 0,
        source: Source::Steam,
        playtime_unknown,
        targets: Vec::new(),
    }
}

fn local_date(ts: Timestamp, tz: &dyn Zone) -> Date {
    ts.to_zoned(tz.zone().clone()).date()
}

fn start_of_week(now: Timestamp, tz: &dyn Zone) -> Date {
    let today = local_date(now, tz);
    let since_monday = i64::from(today.weekday().to_monday_one_offset()) - 1;
    today
        .checked_sub(Span::new().days(since_monday))
        .unwrap_or(today)
}

fn start_of_month(now: Timestamp, tz: &dyn Zone) -> Date {
    local_date(now, tz).first_of_month()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use jiff::tz::TimeZone;

    use super::*;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn session(target: &str, start: &str, end: &str, kind: End) -> Session {
        Session::new(target.to_owned(), ts(start), ts(end), kind)
    }

    #[test]
    fn append_and_read_round_trip_sessions() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("stats/game.jsonl");
        let a = session(
            "game",
            "2026-09-20T19:02:11Z",
            "2026-09-20T23:53:31Z",
            End::Exited,
        );
        let b = session(
            "game",
            "2026-09-21T10:00:00Z",
            "2026-09-21T10:00:05Z",
            End::Failed,
        );
        append(&path, &a).unwrap();
        append(&path, &b).unwrap();
        let log = read(&path).unwrap();
        assert_eq!(log.sessions, vec![a, b]);
        assert_eq!(log.imported, None);
        assert!(log.warnings.is_empty());
    }

    #[test]
    fn a_missing_log_reads_as_empty() {
        let root = tempfile::tempdir().unwrap();
        let log = read(&root.path().join("stats/absent.jsonl")).unwrap();
        assert_eq!(log, Log::default());
    }

    #[test]
    fn a_torn_last_line_is_skipped_with_a_warning() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("game.jsonl");
        let good = session(
            "game",
            "2026-09-20T19:00:00Z",
            "2026-09-20T20:00:00Z",
            End::Exited,
        );
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            format!(
                "{}\n{{\"target\":\"game\",\"started_at\":\"2026-09-21T09:00:00Z\"",
                serde_json::to_string(&good).unwrap()
            ),
        )
        .unwrap();
        let log = read(&path).unwrap();
        assert_eq!(log.sessions, vec![good]);
        assert_eq!(log.warnings.len(), 1);
        assert!(log.warnings[0].contains("line 2"));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("game.jsonl");
        fs::write(
            &path,
            "{\"target\":\"game\",\"started_at\":\"2026-09-20T19:00:00Z\",\"ended_at\":\"2026-09-20T20:00:00Z\",\"seconds\":3600,\"end\":\"exited\",\"new_field\":42}\n",
        )
        .unwrap();
        let log = read(&path).unwrap();
        assert_eq!(log.sessions.len(), 1);
        assert!(log.warnings.is_empty());
    }

    #[test]
    fn importing_twice_appends_another_line_and_the_summary_uses_the_last() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("game.jsonl");
        let before = session(
            "game",
            "2026-09-20T19:00:00Z",
            "2026-09-20T20:00:00Z",
            End::Exited,
        );
        let after = session(
            "game",
            "2026-09-22T19:00:00Z",
            "2026-09-22T20:00:00Z",
            End::Exited,
        );
        append(&path, &before).unwrap();
        append_line(
            &path,
            &ImportedLine {
                imported: Imported {
                    seconds: 342_000,
                    last_played: Some(ts("2026-06-01T10:00:00Z")),
                },
            },
        )
        .unwrap();
        append(&path, &after).unwrap();
        append_line(
            &path,
            &ImportedLine {
                imported: Imported {
                    seconds: 400_000,
                    last_played: None,
                },
            },
        )
        .unwrap();

        // Append-only: both imported lines are still on disk, with the
        // sessions on either side of them kept too.
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 4, "{text}");
        assert_eq!(
            text.lines()
                .filter(|line| line.contains("imported"))
                .count(),
            2,
            "{text}"
        );

        let log = read(&path).unwrap();
        assert_eq!(log.sessions, vec![before, after]);
        // The last `imported` line wins.
        assert_eq!(
            log.imported,
            Some(Imported {
                seconds: 400_000,
                last_played: None,
            })
        );
    }

    #[test]
    fn summary_of_an_empty_log_has_nothing() {
        let summary = summarize(
            &Log::default(),
            None,
            ts("2026-09-26T12:00:00Z"),
            &TimeZone::UTC,
        );
        assert_eq!(summary.seconds, 0);
        assert_eq!(summary.sessions, 0);
        assert_eq!(summary.average_seconds, None);
        assert_eq!(summary.longest, None);
        assert_eq!(summary.first_played, None);
        assert_eq!(summary.last_played, None);
        assert!(!summary.running);
        assert!(summary.targets.is_empty());
    }

    #[test]
    fn summary_adds_totals_average_and_the_longest_session() {
        let log = Log {
            sessions: vec![
                session(
                    "game",
                    "2026-09-12T20:14:00Z",
                    "2026-09-12T21:14:00Z",
                    End::Exited,
                ),
                session(
                    "game",
                    "2026-09-20T19:02:11Z",
                    "2026-09-20T23:53:31Z",
                    End::Exited,
                ),
                session(
                    "editor",
                    "2026-09-25T09:00:00Z",
                    "2026-09-25T09:13:00Z",
                    End::Stopped,
                ),
            ],
            imported: None,
            warnings: Vec::new(),
        };
        let now = ts("2026-09-26T12:00:00Z");
        let summary = summarize(&log, None, now, &TimeZone::UTC);
        assert_eq!(summary.sessions, 3);
        assert_eq!(summary.seconds, 3600 + 17480 + 780);
        assert_eq!(summary.average_seconds, Some((3600 + 17480 + 780) / 3));
        assert_eq!(
            summary.longest,
            Some(Longest {
                seconds: 17480,
                started_at: ts("2026-09-20T19:02:11Z"),
            })
        );
        assert_eq!(summary.first_played, Some(ts("2026-09-12T20:14:00Z")));
        assert_eq!(summary.last_played, Some(ts("2026-09-25T09:00:00Z")));
        assert_eq!(
            summary.targets,
            vec![
                TargetSummary {
                    target: "game".to_owned(),
                    seconds: 3600 + 17480,
                    sessions: 2,
                },
                TargetSummary {
                    target: "editor".to_owned(),
                    seconds: 780,
                    sessions: 1,
                },
            ]
        );
    }

    #[test]
    fn a_running_session_counts_toward_the_numbers_so_far() {
        let log = Log::default();
        let now = ts("2026-09-26T12:00:00Z");
        let running = Running {
            target: "game".to_owned(),
            started_at: ts("2026-09-26T10:00:00Z"),
        };
        let summary = summarize(&log, Some(&running), now, &TimeZone::UTC);
        assert!(summary.running);
        assert_eq!(summary.sessions, 1);
        assert_eq!(summary.seconds, 7200);
        assert_eq!(summary.last_played, Some(running.started_at));
        assert_eq!(summary.longest.unwrap().seconds, 7200);
    }

    #[test]
    fn imported_time_counts_toward_the_total_but_not_sessions_or_average() {
        let log = Log {
            sessions: vec![session(
                "game",
                "2026-09-20T19:00:00Z",
                "2026-09-20T20:00:00Z",
                End::Exited,
            )],
            imported: Some(Imported {
                seconds: 342_000,
                last_played: Some(ts("2026-06-01T10:00:00Z")),
            }),
            warnings: Vec::new(),
        };
        let now = ts("2026-09-26T12:00:00Z");
        let summary = summarize(&log, None, now, &TimeZone::UTC);
        assert_eq!(summary.sessions, 1);
        assert_eq!(summary.average_seconds, Some(3600));
        assert_eq!(summary.imported_seconds, 342_000);
        assert_eq!(summary.seconds, 3600 + 342_000);
        assert_eq!(summary.last_played, Some(ts("2026-09-20T19:00:00Z")));
    }

    #[test]
    fn heroic_last_played_counts_when_it_is_the_latest() {
        let log = Log {
            sessions: Vec::new(),
            imported: Some(Imported {
                seconds: 342_000,
                last_played: Some(ts("2026-06-01T10:00:00Z")),
            }),
            warnings: Vec::new(),
        };
        let summary = summarize(&log, None, ts("2026-09-26T12:00:00Z"), &TimeZone::UTC);
        assert_eq!(summary.last_played, Some(ts("2026-06-01T10:00:00Z")));
        assert_eq!(summary.first_played, None);
    }

    #[test]
    fn this_week_and_month_bucket_by_local_start_date() {
        // A Saturday: the week starts on Monday the 21st, the month on the 1st.
        let now = ts("2026-09-26T12:00:00Z");
        let log = Log {
            sessions: vec![
                session(
                    "game",
                    "2026-09-05T10:00:00Z",
                    "2026-09-05T11:00:00Z",
                    End::Exited,
                ),
                session(
                    "game",
                    "2026-09-22T10:00:00Z",
                    "2026-09-22T11:30:00Z",
                    End::Exited,
                ),
                session(
                    "game",
                    "2026-09-26T08:00:00Z",
                    "2026-09-26T08:15:00Z",
                    End::Exited,
                ),
            ],
            imported: None,
            warnings: Vec::new(),
        };
        let summary = summarize(&log, None, now, &TimeZone::UTC);
        assert_eq!(summary.this_month_seconds, 3600 + 5400 + 900);
        assert_eq!(summary.this_week_seconds, 5400 + 900);
    }

    #[test]
    fn overview_keeps_name_order_and_sorts_played_rows() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let paths = Paths::from_lookup(move |key| {
            (key == "HOME").then(|| OsString::from(home.as_os_str()))
        })
        .unwrap();
        fs::create_dir_all(paths.environments_dir()).unwrap();
        for name in ["alpha", "beta", "never", "zeta"] {
            fs::write(paths.environments_dir().join(format!("{name}.toml")), "").unwrap();
        }
        for (name, seconds) in [("alpha", 3600), ("beta", 7200), ("zeta", 7200)] {
            let start = ts("2026-09-20T19:00:00Z");
            append(
                &paths.stats_log(name),
                &Session {
                    target: "game".to_owned(),
                    started_at: start,
                    ended_at: start + jiff::SignedDuration::from_secs(seconds),
                    seconds,
                    end: End::Exited,
                },
            )
            .unwrap();
        }

        let overview = super::overview(&paths).unwrap();
        assert_eq!(
            overview
                .rows
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta", "never", "zeta"]
        );
        assert_eq!(overview.played, [1, 3, 0]);
        assert_eq!(overview.never_played, 1);
        assert_eq!(overview.total_seconds, 18_000);
        assert!(overview.errors.is_empty());
    }

    /// Steam under `paths`' home, signed in as account 1, with these
    /// `(app id, minutes)` played.
    fn steam_played(paths: &Paths, played: &[(&str, u64)]) {
        let root = paths.home().join(".local/share/Steam");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(root.join("userdata/1/config")).unwrap();
        fs::write(
            root.join("config/loginusers.vdf"),
            "\"users\" { \"76561197960265729\" { \"MostRecent\" \"1\" } }",
        )
        .unwrap();
        let apps: String = played
            .iter()
            .map(|(app_id, minutes)| {
                format!("\"{app_id}\" {{ \"Playtime\" \"{minutes}\" \"LastPlayed\" \"5\" }} ")
            })
            .collect();
        fs::write(
            root.join("userdata/1/config/localconfig.vdf"),
            format!(
                "\"UserLocalConfigStore\" {{ \"Software\" {{ \"Valve\" {{ \"Steam\" {{ \"apps\" {{ {apps} }} }} }} }} }}"
            ),
        )
        .unwrap();
    }

    #[test]
    fn an_overview_reads_each_file_once_however_many_steam_games_it_has() {
        for options in [
            OverviewOptions::default(),
            OverviewOptions {
                load_tags: true,
                ..OverviewOptions::default()
            },
        ] {
            let root = tempfile::tempdir().unwrap();
            let paths = tag_paths(root.path());
            fs::create_dir_all(paths.environments_dir()).unwrap();
            for (name, app_id) in [("one", "10"), ("two", "20"), ("three", "30")] {
                fs::write(
                    paths.environment_config(name),
                    format!("kind = 'proxied'\nlauncher = 'steam'\napp_id = '{app_id}'\n"),
                )
                .unwrap();
            }
            tagged_environment(&paths, "wine", &[]);
            steam_played(&paths, &[("10", 60), ("20", 120)]);

            crate::read_log::clear();
            let overview = overview_with_options(&paths, options).unwrap();

            let seconds: Vec<_> = overview
                .rows
                .iter()
                .map(|(name, summary)| (name.as_str(), summary.seconds, summary.source))
                .collect();
            assert_eq!(
                seconds,
                [
                    ("one", 3600, Source::Steam),
                    ("three", 0, Source::Steam),
                    ("two", 7200, Source::Steam),
                    ("wine", 0, Source::Wineo),
                ]
            );
            assert_eq!(overview.steam_games, 3);
            // Steam's files and the global config are read for the whole
            // overview, and each environment's config once.
            for (file, reads) in [
                ("loginusers.vdf", 1),
                ("localconfig.vdf", 1),
                ("config.toml", 1),
                ("one.toml", 1),
                ("two.toml", 1),
                ("three.toml", 1),
                ("wine.toml", 1),
            ] {
                assert_eq!(
                    crate::read_log::count(file),
                    reads,
                    "{file} with load_tags {}",
                    options.load_tags
                );
            }
        }
    }

    fn tag_paths(root: &Path) -> Paths {
        let home = root.join("home");
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap()
    }

    fn tagged_environment(paths: &Paths, name: &str, tags: &[&str]) {
        fs::create_dir_all(paths.environments_dir()).unwrap();
        fs::write(
            paths.environment_config(name),
            format!("kind = 'native'\ntags = {tags:?}\n"),
        )
        .unwrap();
    }

    #[test]
    fn tag_breakdown_groups_membership_playtime_and_imports() {
        let root = tempfile::tempdir().unwrap();
        let paths = tag_paths(root.path());
        for (name, tags) in [
            ("hades", vec!["game", "co-op"]),
            ("celeste", vec!["game"]),
            ("never", vec!["game"]),
            ("editor", vec!["work"]),
            ("browser", vec![]),
            ("untouched", vec![]),
        ] {
            tagged_environment(&paths, name, &tags);
        }
        let start = ts("2026-09-20T19:00:00Z");
        append(
            &paths.stats_log("hades"),
            &session(
                "game",
                "2026-09-20T19:00:00Z",
                "2026-09-20T20:00:00Z",
                End::Exited,
            ),
        )
        .unwrap();
        set_imported(
            &paths,
            "celeste",
            Imported {
                seconds: 7200,
                last_played: Some(start),
            },
        )
        .unwrap();
        set_imported(
            &paths,
            "browser",
            Imported {
                seconds: 18_000,
                last_played: None,
            },
        )
        .unwrap();

        let overview = overview_with_options(
            &paths,
            OverviewOptions {
                load_tags: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(overview.errors.is_empty());
        let breakdown = by_tag(&overview);
        assert_eq!(
            breakdown
                .tags
                .iter()
                .map(|(tag, _)| tag.as_str())
                .collect::<Vec<_>>(),
            ["game", "co-op", "work"]
        );
        assert_eq!(
            breakdown.tags[0].1,
            TagTotals {
                seconds: 10_800,
                sessions: 1,
                environments: 3,
                played_environments: 2,
                last_played: Some(start)
            }
        );
        assert_eq!(
            breakdown.tags[1].1,
            TagTotals {
                seconds: 3600,
                sessions: 1,
                environments: 1,
                played_environments: 1,
                last_played: Some(start)
            }
        );
        assert_eq!(
            breakdown.tags[2].1,
            TagTotals {
                environments: 1,
                ..Default::default()
            }
        );
        assert_eq!(
            breakdown.untagged,
            Some(TagTotals {
                seconds: 18_000,
                sessions: 0,
                environments: 2,
                played_environments: 1,
                last_played: None
            })
        );
    }

    #[test]
    fn tag_filter_requires_every_tag_and_breakdown_sorts_ties_by_name() {
        let root = tempfile::tempdir().unwrap();
        let paths = tag_paths(root.path());
        tagged_environment(&paths, "both", &["game", "co-op"]);
        tagged_environment(&paths, "game-only", &["game"]);
        let tags = vec!["game".to_owned(), "co-op".to_owned()];
        let overview = overview_with_options(
            &paths,
            OverviewOptions {
                tags: Some(&tags),
                load_tags: false,
            },
        )
        .unwrap();
        assert_eq!(overview.rows.len(), 1);
        assert_eq!(overview.rows[0].0, "both");
        assert_eq!(overview.never_played, 1);
        assert_eq!(overview.total_seconds, 0);
        let breakdown = by_tag(&overview);
        assert_eq!(
            breakdown
                .tags
                .iter()
                .map(|(tag, _)| tag.as_str())
                .collect::<Vec<_>>(),
            ["co-op", "game"]
        );
        assert!(breakdown.untagged.is_none());
    }

    #[test]
    fn configs_are_required_only_for_tag_queries() {
        let root = tempfile::tempdir().unwrap();
        let paths = tag_paths(root.path());
        tagged_environment(&paths, "broken", &[]);
        fs::write(paths.environment_config("broken"), "invalid = [").unwrap();
        let plain = overview(&paths).unwrap();
        assert_eq!(plain.rows.len(), 1);
        assert!(plain.errors.is_empty());
        assert!(plain.tags.is_empty());
        let tags = vec!["game".to_owned()];
        for options in [
            OverviewOptions {
                tags: Some(&tags),
                load_tags: false,
            },
            OverviewOptions {
                tags: None,
                load_tags: true,
            },
        ] {
            let overview = overview_with_options(&paths, options).unwrap();
            assert!(overview.rows.is_empty());
            assert_eq!(overview.errors.len(), 1);
            assert_eq!(overview.errors[0].name, "broken");
            assert!(overview.errors[0].message.contains("broken.toml"));
        }
    }

    #[test]
    fn tag_breakdown_includes_a_running_session() {
        let root = tempfile::tempdir().unwrap();
        let paths = tag_paths(root.path());
        tagged_environment(&paths, "live", &["game"]);
        let runtime = Runtime::new(&paths, "live");
        let _lock = runtime.try_lock().unwrap().unwrap();
        let mut run = crate::runtime::RunState::running(
            crate::runtime::RunMode::Detached,
            "game".to_owned(),
            std::process::id(),
            None,
            &Default::default(),
        );
        run.started_at = Timestamp::now() - jiff::SignedDuration::from_secs(7200);
        runtime.write_state(&run).unwrap();
        let overview = overview_with_options(
            &paths,
            OverviewOptions {
                load_tags: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(overview.rows[0].1.running);
        let breakdown = by_tag(&overview);
        assert_eq!(
            breakdown.tags[0].1,
            TagTotals {
                seconds: overview.now.duration_since(run.started_at).as_secs(),
                sessions: 1,
                environments: 1,
                played_environments: 1,
                last_played: Some(run.started_at)
            }
        );
    }

    #[test]
    fn last_session_duration_uses_the_session_named_by_last_played() {
        let now = ts("2026-09-26T12:00:00Z");
        let log = Log {
            sessions: vec![
                session(
                    "game",
                    "2026-09-24T10:00:00Z",
                    "2026-09-24T11:00:00Z",
                    End::Exited,
                ),
                session(
                    "game",
                    "2026-09-25T10:00:00Z",
                    "2026-09-25T10:30:00Z",
                    End::Exited,
                ),
            ],
            ..Log::default()
        };
        let summary = summarize(&log, None, now, &TimeZone::UTC);
        assert_eq!(last_session_seconds(&log, &summary, now), Some(1800));

        let running = Running {
            target: "game".to_owned(),
            started_at: ts("2026-09-26T11:45:00Z"),
        };
        let summary = summarize(&log, Some(&running), now, &TimeZone::UTC);
        assert_eq!(last_session_seconds(&log, &summary, now), Some(900));
    }

    #[test]
    fn copied_log_never_replaces_one_already_there() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let lookup = move |key: &str| (key == "HOME").then(|| OsString::from(&home));
        let from = Paths::select(lookup.clone(), None).unwrap();
        let to = Paths::select(lookup, Some(&root.path().join("usb"))).unwrap();
        fs::create_dir_all(from.stats_log("game").parent().unwrap()).unwrap();
        fs::write(from.stats_log("game"), "source\n").unwrap();
        fs::create_dir_all(to.stats_log("game").parent().unwrap()).unwrap();
        fs::write(to.stats_log("game"), "orphan\n").unwrap();
        assert!(copy_log(&from, &to, "game").is_err());
        assert_eq!(
            fs::read_to_string(to.stats_log("game")).unwrap(),
            "orphan\n"
        );
        assert_eq!(
            fs::read_dir(to.stats_log("game").parent().unwrap())
                .unwrap()
                .count(),
            1
        );
    }
}
