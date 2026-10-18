//! Playtime tracking: an append-only session log per environment.
//!
//! `$XDG_DATA_HOME/wineo/stats/<env>.jsonl`, or `<folder>/stats/<env>.jsonl`
//! in a portable folder (see `Paths::stats_log`). One JSON object per line: a
//! session, or an `imported` line Heroic playtime is carried over as. The log
//! is strictly append-only, including a re-import: reading keeps the last
//! `imported` line it sees. Totals are computed on read, so nothing has to
//! stay in sync.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jiff::civil::Date;
use jiff::{Span, Timestamp};
use serde::{Deserialize, Serialize};

use crate::environment::{self, Environment};
use crate::error::{IoContext, Result};
use crate::files;
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
    let Some(bytes) =
        files::read_optional(&source).context(|| format!("failed to read {}", source.display()))?
    else {
        return Ok(None);
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
    files::sync_dir(parent)
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
    let Some(text) = files::read_to_string_optional(path)
        .context(|| format!("failed to read {}", path.display()))?
    else {
        return Ok(Log::default());
    };
    Ok(parse(path, &text))
}

fn parse(path: &Path, text: &str) -> Log {
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
    log
}

/// Parsed play logs kept between overviews, for a caller that builds one
/// every few seconds (the GUI's rescan). Only the parse is kept, never a
/// `Summary`: those depend on the clock and on the live run, so they are
/// recomputed from the cached sessions on every call.
#[derive(Debug, Default)]
pub struct Cache {
    logs: HashMap<PathBuf, CachedLog>,
    /// How many logs were read and parsed, for tests that pin a cache hit.
    #[cfg(any(test, feature = "test-support"))]
    parses: usize,
}

#[derive(Debug)]
struct CachedLog {
    stamp: files::Stamp,
    log: Arc<Log>,
}

impl Cache {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn parses(&self) -> usize {
        self.parses
    }

    /// The log at `path`, parsed only if its file changed since the cached
    /// parse. Anything unusual (no file, an unreadable one, a file that
    /// changed while being read) is read and returned as `read` would and
    /// leaves nothing cached, so errors and warnings match an uncached read.
    fn read(&mut self, path: &Path) -> Result<Arc<Log>> {
        if let Some(stamp) = files::Stamp::of(path).ok().flatten()
            && let Some(cached) = self.logs.get(path)
            && cached.stamp == stamp
        {
            return Ok(Arc::clone(&cached.log));
        }
        self.logs.remove(path);
        #[cfg(any(test, feature = "test-support"))]
        {
            self.parses += 1;
        }
        let Some((bytes, stamp)) =
            files::read_stamped(path).context(|| format!("failed to read {}", path.display()))?
        else {
            return Ok(Arc::new(Log::default()));
        };
        let Ok(text) = String::from_utf8(bytes) else {
            // Not text: read it the plain way, which reports the same error.
            return read(path).map(Arc::new);
        };
        let log = Arc::new(parse(path, &text));
        if let Some(stamp) = stamp {
            self.logs.insert(
                path.to_owned(),
                CachedLog {
                    stamp,
                    log: Arc::clone(&log),
                },
            );
        }
        Ok(log)
    }
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

/// `overview` that re-parses only the logs that changed since `cache` last
/// saw them, and drops the entries of environments that are gone.
pub fn overview_cached(paths: &Paths, cache: &mut Cache) -> Result<Overview> {
    overview_cached_at(paths, cache, Timestamp::now())
}

/// `overview_cached` as of `now`, so a test can move the clock.
fn overview_cached_at(paths: &Paths, cache: &mut Cache, now: Timestamp) -> Result<Overview> {
    overview_inner(paths, OverviewOptions::default(), Some(cache), now)
}

/// Loads configs only when filtering or collecting tags for a breakdown.
pub fn overview_with_options(paths: &Paths, options: OverviewOptions<'_>) -> Result<Overview> {
    overview_inner(paths, options, None, Timestamp::now())
}

fn overview_inner(
    paths: &Paths,
    options: OverviewOptions<'_>,
    mut cache: Option<&mut Cache>,
    now: Timestamp,
) -> Result<Overview> {
    let tz = LocalZone::new();
    let mut rows = Vec::new();
    let mut tags = BTreeMap::new();
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    // One reading of Steam's files for every Steam game in the overview.
    let steam = SteamPlaytimes::new(paths);
    let names = environment::names(paths)?;
    if let Some(cache) = cache.as_deref_mut() {
        // Every name counts, including ones a tag filter skips, so a filtered
        // overview doesn't evict the logs an unfiltered one will want.
        let live: HashSet<PathBuf> = names.iter().map(|name| paths.stats_log(name)).collect();
        cache.logs.retain(|path, _| live.contains(path));
    }
    let loads_configs = options.tags.is_some() || options.load_tags;
    // Nothing here trashes or restores, so one trash scan serves every load.
    let pending = loads_configs.then(|| crate::trash::PendingScan::new(paths));
    for name in names {
        let (environment_tags, loaded) = if let Some(pending) = &pending {
            match Environment::load_with_pending(paths, &name, pending) {
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
        let summary = match cache.as_deref_mut() {
            Some(cache) => summary_cached(paths, &name, config.as_ref(), &steam, cache, now, &tz),
            None => summary_of(paths, &name, config.as_ref(), &steam, now, &tz)
                .map(|(summary, _, warns)| (summary, warns)),
        };
        match summary {
            Ok((summary, warns)) => {
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
        // Each import appends to a play log a removal could be moving, so the
        // trash is checked right before it rather than once for the batch.
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
    if let Some(summary) = steam_summary(config, steam) {
        return Ok((summary, None, Vec::new()));
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

/// `summary_of` for an overview with a cache: the log comes from the cache
/// when its file is unchanged, and the summary is built fresh either way.
fn summary_cached(
    paths: &Paths,
    name: &str,
    config: Option<&crate::config::EnvironmentConfig>,
    steam: &SteamPlaytimes<'_>,
    cache: &mut Cache,
    now: Timestamp,
    tz: &dyn Zone,
) -> Result<(Summary, Vec<String>)> {
    if let Some(summary) = steam_summary(config, steam) {
        return Ok((summary, Vec::new()));
    }
    let log = cache.read(&paths.stats_log(name))?;
    let running = running_session(paths, name)?;
    Ok((
        summarize(&log, running.as_ref(), now, tz),
        log.warnings.clone(),
    ))
}

/// The summary of a Steam-backed environment, or `None` for a Wineo one.
fn steam_summary(
    config: Option<&crate::config::EnvironmentConfig>,
    steam: &SteamPlaytimes<'_>,
) -> Option<Summary> {
    let config = config?;
    let crate::config::Kind::Proxied(proxied) = &config.kind else {
        return None;
    };
    Some(summarize_steam(steam.playtime(&proxied.app_id)))
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
mod tests;
