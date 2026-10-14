use jiff::Timestamp;
use wineo_core::operation::Operation;
use wineo_core::report::{InstallReport, KindReport, Overview};
use wineo_core::runtime::{State, Status};
use wineo_core::view::{CollectionError, CollectionOutput, StatusOutput};

use wineo_core::time::{Zone, clock, relative};

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
    hooks: Option<wineo_core::view::HooksOutput>,
    kind: Option<KindReport>,
}

impl Row {
    fn new(
        name: &str,
        status: &Status,
        busy: Option<Operation>,
        unresolved: bool,
        tags: &[String],
        now: Timestamp,
        tz: &dyn Zone,
    ) -> Self {
        let state = match busy {
            Some(operation @ Operation::Hook(_)) => operation.describe(),
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
            hooks: None,
            kind: None,
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
                .map(|row| StatusOutput {
                    hooks: row.hooks.clone(),
                    ..StatusOutput::new(
                        &row.name,
                        &row.status,
                        row.busy,
                        row.unresolved,
                        &row.tags,
                        row.kind.as_ref(),
                    )
                })
                .collect(),
            errors: &self.errors,
        }
    }
}

pub fn format_overview(overview: Overview, now: Timestamp, tz: &dyn Zone) -> Gathered {
    let rows = overview
        .environments
        .iter()
        .map(|environment| {
            let mut row = Row::new(
                &environment.name,
                &environment.status,
                environment.busy,
                environment.unresolved,
                &environment.tags,
                now,
                tz,
            );
            row.hooks = environment.hooks.clone();
            row.kind = environment.kind.clone();
            if environment.busy.is_none()
                && !environment.unresolved
                && let Some(install) = row
                    .kind
                    .as_ref()
                    .and_then(|kind| unfinished_install(environment.status.state, kind))
            {
                row.state = install.label();
            }
            row
        })
        .collect();
    let errors = overview
        .errors
        .into_iter()
        .map(|error| CollectionError {
            name: error.name,
            message: error.message,
        })
        .collect();
    Gathered { rows, errors }
}

/// The install of a Steam game to show in place of its run state, when Steam
/// isn't running it and the game isn't installed yet.
pub fn unfinished_install(state: State, kind: &KindReport) -> Option<&InstallReport> {
    match kind {
        KindReport::Proxied { install, .. }
            if !matches!(
                state,
                State::Starting | State::Running | State::SteamRunning
            ) && install.state != "installed" =>
        {
            Some(install)
        }
        _ => None,
    }
}

fn time_column(status: &Status, now: Timestamp, tz: &dyn Zone) -> String {
    let Some(run) = &status.run else {
        return String::new();
    };
    match status.state {
        State::Running | State::Starting => format!(
            "since {} ({})",
            clock(now, run.started_at, tz),
            relative(now, run.started_at)
        ),
        State::Failed if status.lost => format!("started {}", relative(now, run.started_at)),
        State::Exited | State::Stopped | State::Failed => {
            let ended = run.ended_at.unwrap_or(run.started_at);
            format!("ended {}", relative(now, ended))
        }
        State::Inactive | State::SteamRunning => String::new(),
    }
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

/// Width of the label column in the detail lines of `status <name>` and
/// `stats <name>`, wide enough for the longest label either has.
pub const LABEL_WIDTH: usize = 10;
/// Spaces before the label, and between the label and its value.
pub const LABEL_INDENT: usize = 2;
pub const LABEL_GAP: usize = 2;

pub fn label_line(label: &str, value: &str) -> String {
    format!(
        "{}{:width$}{}{value}",
        " ".repeat(LABEL_INDENT),
        label,
        " ".repeat(LABEL_GAP),
        width = LABEL_WIDTH,
    )
}

/// Indented `label  value` lines with the labels padded to the longest.
pub fn aligned_rows(rows: Vec<(String, String)>) -> Vec<String> {
    let width = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    rows.into_iter()
        .map(|(label, value)| format!("  {label:<width$}  {value}"))
        .collect()
}

/// The width of a label column whose values follow directly, two spaces past
/// the longest label.
pub fn label_column<'a>(labels: impl IntoIterator<Item = &'a str>) -> usize {
    labels.into_iter().map(str::len).max().unwrap_or(0) + 2
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use jiff::tz::TimeZone;
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
            launcher: None,
            status,
            lost: false,
            phase: None,
            mode: RunMode::Detached,
            target: target.to_owned(),
            pid: 1,
            group: Some(1),
            boot_id: None,
            leader_start: None,
            started_at: ts("2026-09-16T14:02:00Z"),
            running_at: None,
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
                hooks: None,
                kind: None,
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
                hooks: None,
                kind: None,
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
                hooks: None,
                kind: None,
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
