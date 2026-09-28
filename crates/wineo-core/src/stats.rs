//! Playtime tracking: an append-only session log per environment.
//!
//! `$XDG_DATA_HOME/wineo/stats/<env>.jsonl`, or `<folder>/stats/<env>.jsonl`
//! in a portable folder (see `Paths::stats_log`). One JSON object per line: a
//! session, or an `imported` line Heroic playtime is carried over as. The log
//! is strictly append-only, including a re-import: reading keeps the last
//! `imported` line it sees. Totals are computed on read, so nothing has to
//! stay in sync.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};
use serde::{Deserialize, Serialize};

use crate::environment::{self, Environment};
use crate::error::{IoContext, Result};
use crate::files::write_atomic;
use crate::heroic::HeroicSource;
use crate::paths::Paths;
use crate::runtime::{Runtime, State};

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

/// Removes an environment's whole play log, for `rm`. Returns whether there was one.
pub fn remove_log(paths: &Paths, name: &str) -> Result<bool> {
    let path = paths.stats_log(name);
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).context(|| format!("failed to remove {}", path.display())),
    }
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
    write_atomic(&destination, bytes)?;
    Ok(Some(destination))
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
    /// Every logged session's time, an in-progress one's so far, and whatever
    /// was carried over from Heroic.
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
    /// Sorted by time played, most first.
    pub targets: Vec<TargetSummary>,
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
    /// Indices into `rows`, sorted by time played and then name.
    pub played: Vec<usize>,
    pub total_seconds: i64,
    pub never_played: usize,
    pub warnings: Vec<String>,
    pub errors: Vec<OverviewError>,
    pub now: Timestamp,
    pub tz: TimeZone,
}

/// Collects every environment's playtime without stopping for one bad log.
pub fn overview(paths: &Paths) -> Result<Overview> {
    let now = Timestamp::now();
    let tz = TimeZone::system();
    let mut rows = Vec::new();
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    for name in environment::names(paths)? {
        match summary(paths, &name, now, &tz) {
            Ok((summary, warns)) => {
                warnings.extend(warns);
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
    let never_played = rows.len() - played.len();
    let total_seconds = rows.iter().map(|(_, summary)| summary.seconds).sum();
    Ok(Overview {
        rows,
        played,
        total_seconds,
        never_played,
        warnings,
        errors,
        now,
        tz,
    })
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
pub fn summarize(log: &Log, running: Option<&Running>, now: Timestamp, tz: &TimeZone) -> Summary {
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

    let week_start = start_of_week(now, tz);
    let month_start = start_of_month(now, tz);
    let this_week_seconds = sessions
        .iter()
        .filter(|session| local_date(session.started_at, tz) >= week_start)
        .map(|session| session.seconds)
        .sum();
    let this_month_seconds = sessions
        .iter()
        .filter(|session| local_date(session.started_at, tz) >= month_start)
        .map(|session| session.seconds)
        .sum();

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

/// Reads `name`'s log and folds in its live run, if any, as of `now`.
/// Returns the summary alongside any warnings from unreadable lines.
pub fn summary(
    paths: &Paths,
    name: &str,
    now: Timestamp,
    tz: &TimeZone,
) -> Result<(Summary, Vec<String>)> {
    let log = read(&paths.stats_log(name))?;
    let running = running_session(paths, name)?;
    let warnings = log.warnings.clone();
    Ok((summarize(&log, running.as_ref(), now, tz), warnings))
}

fn local_date(ts: Timestamp, tz: &TimeZone) -> Date {
    ts.to_zoned(tz.clone()).date()
}

fn start_of_week(now: Timestamp, tz: &TimeZone) -> Date {
    let today = local_date(now, tz);
    let since_monday = i64::from(today.weekday().to_monday_one_offset()) - 1;
    today
        .checked_sub(Span::new().days(since_monday))
        .unwrap_or(today)
}

fn start_of_month(now: Timestamp, tz: &TimeZone) -> Date {
    local_date(now, tz).first_of_month()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

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
}
