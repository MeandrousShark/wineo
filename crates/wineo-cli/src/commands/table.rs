use jiff::Timestamp;
use jiff::tz::TimeZone;
use wineo_core::environment::{self, Environment};
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::report;
use wineo_core::runtime::{Runtime, State, Status};

use super::json::{CollectionError, CollectionOutput, StatusOutput};
use super::time::{clock, relative};

/// One line of `wineo status` or `wineo list`: name, state, target, and time.
pub struct Row {
    name: String,
    state: String,
    target: String,
    time: String,
    status: Status,
    busy: Option<Operation>,
    unresolved: bool,
    tags: Vec<String>,
}

impl Row {
    fn new(
        name: &str,
        status: &Status,
        busy: Option<Operation>,
        unresolved: bool,
        tags: &[String],
        now: Timestamp,
        tz: &TimeZone,
    ) -> Self {
        let state = match busy {
            Some(operation) => operation.as_str(),
            None if unresolved => "unresolved",
            None => status.state.as_str(),
        };
        Self {
            name: name.to_owned(),
            state: state.to_owned(),
            target: status
                .run
                .as_ref()
                .map(|run| run.target.clone())
                .unwrap_or_default(),
            time: time_column(status, now, tz),
            status: status.clone(),
            busy,
            unresolved,
            tags: tags.to_vec(),
        }
    }
}

pub struct Gathered {
    pub rows: Vec<Row>,
    pub errors: Vec<CollectionError>,
}

impl Gathered {
    pub fn failed(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn print_errors(&self) {
        for error in &self.errors {
            eprintln!("error: {}", error.message);
        }
    }

    pub fn json(&self) -> CollectionOutput<'_> {
        CollectionOutput {
            environments: self
                .rows
                .iter()
                .map(|row| {
                    StatusOutput::new(&row.name, &row.status, row.busy, row.unresolved, &row.tags)
                })
                .collect(),
            errors: &self.errors,
        }
    }
}

fn time_column(status: &Status, now: Timestamp, tz: &TimeZone) -> String {
    let Some(run) = &status.run else {
        return String::new();
    };
    match status.state {
        State::Running => format!(
            "since {} ({})",
            clock(now, run.started_at, tz),
            relative(now, run.started_at)
        ),
        State::Failed if status.lost => format!("started {}", relative(now, run.started_at)),
        State::Exited | State::Stopped | State::Failed => {
            let ended = run.ended_at.unwrap_or(run.started_at);
            format!("ended {}", relative(now, ended))
        }
        State::Inactive => String::new(),
    }
}

/// Gathers a row for every environment name, keeping only the ones `keep`
/// accepts. A tag filter requires a readable config; otherwise unreadable
/// configs still appear with empty tags, as they did before tags were added.
pub fn gather(
    paths: &Paths,
    now: Timestamp,
    tz: &TimeZone,
    tags: Option<&[String]>,
    mut keep: impl FnMut(&Status, bool) -> bool,
) -> anyhow::Result<Gathered> {
    let locks = Locks::new(paths);
    let mut rows = Vec::new();
    let mut errors = Vec::new();
    for name in environment::names(paths)? {
        let busy = locks.busy_with(&name).unwrap_or(None);
        let unresolved = busy.is_none() && report::unresolved(paths, &name);
        match Runtime::new(paths, &name).status() {
            Ok(status) if keep(&status, busy.is_some() || unresolved) => {
                let config = Environment::load(paths, &name);
                match config {
                    Ok(environment) => {
                        let environment_tags = &environment.config.tags;
                        if tags.is_none_or(|filter| {
                            filter.iter().all(|tag| environment_tags.contains(tag))
                        }) {
                            rows.push(Row::new(
                                &name,
                                &status,
                                busy,
                                unresolved,
                                environment_tags,
                                now,
                                tz,
                            ));
                        }
                    }
                    Err(err) if tags.is_some() => errors.push(CollectionError {
                        name,
                        message: format!("{:#}", anyhow::Error::from(err)),
                    }),
                    Err(_) => rows.push(Row::new(&name, &status, busy, unresolved, &[], now, tz)),
                }
            }
            Ok(_) => {}
            Err(err) => {
                errors.push(CollectionError {
                    name,
                    message: format!("{:#}", anyhow::Error::from(err)),
                });
            }
        }
    }
    Ok(Gathered { rows, errors })
}

/// Aligns columns, padded to the widest value in that column with two spaces
/// between them, with trailing spaces trimmed off each line.
pub fn render(rows: &[Row]) -> Vec<String> {
    let columns: Vec<_> = rows
        .iter()
        .map(|row| {
            [
                row.name.clone(),
                row.state.clone(),
                row.target.clone(),
                row.time.clone(),
            ]
        })
        .collect();
    render_columns(&columns)
}

/// Aligns arbitrary fixed-width rows with two spaces between columns.
pub fn render_columns<const N: usize>(rows: &[[String; N]]) -> Vec<String> {
    let widths: [usize; N] =
        std::array::from_fn(|column| rows.iter().map(|row| row[column].len()).max().unwrap_or(0));
    rows.iter()
        .map(|row| {
            let mut line = String::new();
            for (column, value) in row.iter().enumerate() {
                if column > 0 {
                    line.push_str("  ");
                }
                line.push_str(value);
                if column + 1 < N {
                    line.push_str(&" ".repeat(widths[column] - value.len()));
                }
            }
            line.trim_end().to_owned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use wineo_core::runtime::{RunMode, RunState, RunStatus};

    use super::*;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn run(status: RunStatus, target: &str) -> RunState {
        RunState {
            runner: None,
            prefix: None,
            native: false,
            status,
            mode: RunMode::Detached,
            target: target.to_owned(),
            pid: 1,
            group: Some(1),
            boot_id: None,
            leader_start: None,
            started_at: ts("2026-09-16T14:02:00Z"),
            heartbeat_at: None,
            program_exited_at: None,
            ended_at: None,
            exit_code: None,
            requested_target: None,
            args: Vec::new(),
            env: BTreeMap::new(),
        }
    }

    #[test]
    fn time_column_covers_every_state() {
        let now = ts("2026-09-16T14:05:00Z");
        let tz = TimeZone::UTC;

        let running = Status {
            state: State::Running,
            run: Some(run(RunStatus::Running, "notepad")),
            lost: false,
        };
        assert_eq!(time_column(&running, now, &tz), "since 14:02 (3 min ago)");

        let starting = Status {
            state: State::Running,
            run: None,
            lost: false,
        };
        assert_eq!(time_column(&starting, now, &tz), "");

        let mut exited_run = run(RunStatus::Exited, "notepad");
        exited_run.ended_at = Some(ts("2026-09-15T22:05:00Z"));
        let exited = Status {
            state: State::Exited,
            run: Some(exited_run),
            lost: false,
        };
        assert_eq!(time_column(&exited, now, &tz), "ended 16 h ago");

        let lost = Status {
            state: State::Failed,
            run: Some(run(RunStatus::Running, "notepad")),
            lost: true,
        };
        assert_eq!(time_column(&lost, now, &tz), "started 3 min ago");

        let inactive = Status {
            state: State::Inactive,
            run: None,
            lost: false,
        };
        assert_eq!(time_column(&inactive, now, &tz), "");
    }

    #[test]
    fn render_pads_columns_and_trims_trailing_spaces() {
        let rows = vec![
            Row {
                name: "notepad".to_owned(),
                state: "running".to_owned(),
                target: "notepad".to_owned(),
                time: "since 14:02 (3 min ago)".to_owned(),
                status: Status {
                    state: State::Running,
                    run: None,
                    lost: false,
                },
                busy: None,
                unresolved: false,
                tags: Vec::new(),
            },
            Row {
                name: "photoshop".to_owned(),
                state: "exited".to_owned(),
                target: "photoshop".to_owned(),
                time: "ended 16 h ago".to_owned(),
                status: Status {
                    state: State::Exited,
                    run: None,
                    lost: false,
                },
                busy: None,
                unresolved: false,
                tags: Vec::new(),
            },
            Row {
                name: "winecfg".to_owned(),
                state: "inactive".to_owned(),
                target: String::new(),
                time: String::new(),
                status: Status {
                    state: State::Inactive,
                    run: None,
                    lost: false,
                },
                busy: None,
                unresolved: false,
                tags: Vec::new(),
            },
        ];
        assert_eq!(
            render(&rows),
            [
                "notepad    running   notepad    since 14:02 (3 min ago)",
                "photoshop  exited    photoshop  ended 16 h ago",
                "winecfg    inactive",
            ]
        );
    }
}
