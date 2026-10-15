//! Galaxy builds in `wineo import gog --game` and `wineo update`: the rows
//! of their reviews, progress lines, and what a finished run says. The
//! wording lives here, apart from the flow that uses it.

use std::time::{Duration, Instant};

use wineo_core::cancel::Token;
use wineo_core::gog::folder::{JournalKind, ProductStatus, Track};
use wineo_core::gog::update::galaxy::Progress;
use wineo_core::gog::update::galaxy::update::{Note, Plan};
use wineo_core::gog::update::{SavedAside, Updated};
use wineo_core::paths::Paths;
use wineo_core::units;

use super::table;

const SPACE_UNCOUNTED: &str = "Setup's own writes in the prefix aren't counted in the space check.";
const NOTHING_DELETED_MOVE: &str =
    "Nothing is deleted: files the installer left that this build doesn't list stay.";
const NOTHING_DELETED: &str =
    "Nothing is deleted: files of the installed build that this one doesn't list stay.";

/// What `--track` takes.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum GogTrack {
    /// Galaxy's builds, file by file
    Galaxy,
    /// The offline installers
    Installer,
}

impl From<GogTrack> for Track {
    fn from(track: GogTrack) -> Self {
        match track {
            GogTrack::Galaxy => Track::Galaxy,
            GogTrack::Installer => Track::Installer,
        }
    }
}

/// A count with thousands separators, like `1,421`.
pub fn thousands(count: usize) -> String {
    let digits = count.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// A build as a person reads it: GOG's name for it, else its id.
fn version(name: Option<&str>, build: &str) -> String {
    name.filter(|name| !name.is_empty())
        .unwrap_or(build)
        .to_owned()
}

/// The setup steps as a row's value.
pub fn setup_value(steps: &[String], all: bool) -> Option<String> {
    if steps.is_empty() {
        return None;
    }
    let list = steps.join(", ");
    Some(if all {
        format!("{list}, all run again")
    } else {
        list
    })
}

/// `Setup`'s rows shared by an install and an update: the support files,
/// when there are any.
pub fn support_value(files: usize) -> Option<String> {
    (files > 0).then(|| {
        format!(
            "{} {}",
            thousands(files),
            crate::text::plural(files, "file", "files")
        )
    })
}

/// What an update, repair, reinstall, or move reviews, row by row.
fn update_rows(paths: &Paths, plan: &Plan) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    rows.push((
        "folder".to_owned(),
        crate::style::path(paths, plan.folder()),
    ));
    let track = if plan.moves_from().is_some() {
        "installer -> Galaxy"
    } else {
        "Galaxy"
    };
    rows.push(("track".to_owned(), track.to_owned()));
    if plan.claims() {
        rows.push((
            "owner".to_owned(),
            format!("none yet, {} takes this folder", plan.environment()),
        ));
    }
    let labels = plan.product_labels();
    let target = version(plan.version_name(), plan.build());
    for product in plan.products() {
        let label = labels
            .get(&product.id)
            .cloned()
            .unwrap_or_else(|| product.id.to_string());
        let value = if product.status != ProductStatus::Installed {
            "unavailable".to_owned()
        } else if product.id != plan.gog_id() {
            target.clone()
        } else {
            base_change(plan, &target)
        };
        rows.push((label, value));
    }
    if let Some(files) = support_value(plan.support_files()) {
        rows.push(("support".to_owned(), files));
    }
    if plan.kind() == JournalKind::Repair {
        let (files, bytes) = plan.checked();
        rows.push((
            "check".to_owned(),
            format!(
                "every file, {} {} ({})",
                thousands(files),
                crate::text::plural(files, "file", "files"),
                units::size(bytes)
            ),
        ));
    }
    let steps = plan.setup_steps(paths);
    if let Some(value) = setup_value(&steps, plan.hashes_all()) {
        rows.push(("setup".to_owned(), value));
    }
    let download = match plan.kind() {
        JournalKind::Repair => "only what's missing or damaged".to_owned(),
        JournalKind::Reinstall => format!(
            "up to {}, files that already match are kept",
            units::size(plan.download_size())
        ),
        _ => units::size(plan.download_size()),
    };
    rows.push(("download".to_owned(), download));
    rows
}

/// The base game's row: where it was and where it goes.
fn base_change(plan: &Plan, target: &str) -> String {
    if plan.kind() == JournalKind::Repair {
        return target.to_owned();
    }
    let moving = plan.moves_from().is_some();
    let from = if moving {
        plan.installed_version().map(str::to_owned)
    } else {
        plan.installed_build()
            .map(|build| version(plan.installed_version(), build))
    };
    let Some(from) = from else {
        return format!("unknown -> {target}");
    };
    if !moving && plan.installed_build() == Some(plan.build()) {
        return if plan.kind() == JournalKind::Reinstall {
            format!("{target} (rebuilt)")
        } else {
            target.to_owned()
        };
    }
    if from == target {
        // The same name on another build, as when an installer's version
        // moves onto Galaxy.
        return format!("{from} -> {target} (build {})", plan.build());
    }
    format!("{from} -> {target}")
}

/// The lines of an update's review: its title, the rows, and the notes that
/// follow.
pub fn update_review(paths: &Paths, plan: &Plan) -> Vec<String> {
    let mut lines = vec![format!("{} {}", plan.action(), plan.environment())];
    lines.extend(table::aligned_rows(update_rows(paths, plan)));
    for note in plan.notes() {
        lines.push(match note {
            Note::NothingDeleted if plan.moves_from().is_some() => NOTHING_DELETED_MOVE.to_owned(),
            Note::NothingDeleted => NOTHING_DELETED.to_owned(),
            Note::SetupSpaceUncounted => SPACE_UNCOUNTED.to_owned(),
        });
    }
    lines
}

/// A Galaxy install's rows after its platform, language, and runner: the
/// build, DLC, support files, setup, and size.
pub fn install_rows(
    paths: &Paths,
    plan: &wineo_core::gog::update::galaxy::install::Plan,
    rows: &mut Vec<(String, String)>,
) {
    let target = version(plan.version_name(), plan.build());
    let labels = plan.product_labels();
    let mut dlc = Vec::new();
    for product in plan.products() {
        let value = if product.status == ProductStatus::Installed {
            target.clone()
        } else {
            "unavailable".to_owned()
        };
        if product.id == plan.gog_id() {
            rows.push(("version".to_owned(), value));
        } else if let Some(label) = labels.get(&product.id) {
            dlc.push(format!("{label} {value}"));
        }
    }
    // One DLC per line; the rest sit under the first one's value.
    for (index, line) in dlc.into_iter().enumerate() {
        let label = if index == 0 { "dlc" } else { "" };
        rows.push((label.to_owned(), line));
    }
    if let Some(files) = support_value(plan.support_files()) {
        rows.push(("support".to_owned(), files));
    }
    if let Some(value) = setup_value(&plan.setup_steps(paths), false) {
        rows.push(("setup".to_owned(), value));
    }
    rows.push(("download".to_owned(), units::size(plan.installed_size())));
}

/// The rows an update reports as changed: the products it installed, as the
/// review showed them.
pub fn changes(paths: &Paths, plan: &Plan, updated: &Updated) -> Vec<(String, String)> {
    let rows = update_rows(paths, plan);
    let labels = plan.product_labels();
    updated
        .products
        .iter()
        .filter(|product| product.status == ProductStatus::Installed)
        .filter_map(|product| {
            let label = labels.get(&product.id)?;
            rows.iter()
                .find(|(row, _)| row == label)
                .map(|(label, value)| (label.clone(), value.clone()))
        })
        .collect()
}

/// What a finished run says about files it moved aside.
pub fn saved_lines(plan: &Plan, saved: &[SavedAside]) -> Vec<String> {
    if saved.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![
        if matches!(plan.kind(), JournalKind::Repair | JournalKind::Reinstall) {
            "Saved these files aside, since they didn't match GOG's:"
        } else {
            "Moved your files aside where the new version has files:"
        }
        .to_owned(),
    ];
    lines.extend(
        saved
            .iter()
            .map(|saved| format!("  {} -> {}", saved.path, saved.saved_as)),
    );
    lines
}

/// One line for each redistributable that asked for a restart.
pub fn restart_lines(needs_restart: &[String]) -> Vec<String> {
    needs_restart
        .iter()
        .map(|step| format!("{step} asked for a restart, which Wine doesn't need."))
        .collect()
}

/// Turns Galaxy progress into status lines: one per stage, and the byte
/// counts at most once per `interval`.
pub struct Reporter {
    interval: Duration,
    last: Option<Instant>,
    checking: bool,
    downloading: bool,
}

impl Reporter {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
            checking: false,
            downloading: false,
        }
    }

    fn due(&self) -> bool {
        self.last.is_none_or(|last| last.elapsed() >= self.interval)
    }

    fn said(&mut self, line: String) -> Option<String> {
        self.last = Some(Instant::now());
        Some(line)
    }

    /// The line for `step`, or None when it says nothing new yet.
    pub fn line(&mut self, step: &Progress) -> Option<String> {
        match step {
            Progress::Verify {
                files,
                files_total,
                bytes,
                bytes_total,
            } => {
                let first = !std::mem::replace(&mut self.checking, true);
                (first || files == files_total || self.due()).then_some(())?;
                let line = format!(
                    "Checking files: {} / {} ({} / {})",
                    thousands(*files),
                    thousands(*files_total),
                    units::size(*bytes),
                    units::size(*bytes_total)
                );
                self.said(line)
            }
            Progress::Download { done, total } => {
                let first = !std::mem::replace(&mut self.downloading, true);
                (first || done == total || self.due()).then_some(())?;
                let line = format!(
                    "Downloading: {} / {}",
                    units::size(*done),
                    units::size(*total)
                );
                self.said(line)
            }
            Progress::Setup {
                done,
                total,
                step: Some(step),
            } => {
                let line = format!("Running setup {} of {total}: {step}", done + 1);
                self.said(line)
            }
            Progress::Setup { step: None, .. } => None,
        }
    }
}

/// Lets a test stop a Galaxy update at a known point: with
/// `WINEO_TEST_UPDATE_PAUSE=download`, the first download step waits for
/// Ctrl+C, and `download:NAME` only for that environment. Release builds
/// never look at it.
#[cfg(debug_assertions)]
pub fn pause_for_test(environment: &str, step: &Progress, token: &Token) {
    let wanted = std::env::var("WINEO_TEST_UPDATE_PAUSE").unwrap_or_default();
    let (at, only) = wanted.split_once(':').unwrap_or((wanted.as_str(), ""));
    let here = at == "download"
        && matches!(step, Progress::Download { .. })
        && (only.is_empty() || only == environment);
    while here && !token.is_canceled() {
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_get_thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1421), "1,421");
        assert_eq!(thousands(1234567), "1,234,567");
    }

    #[test]
    fn progress_lines_are_throttled_but_stages_and_ends_always_show() {
        let mut reporter = Reporter::new(Duration::from_secs(3600));
        let verify = |files, bytes| Progress::Verify {
            files,
            files_total: 1421,
            bytes,
            bytes_total: 498 * 1024 * 1024,
        };
        assert_eq!(
            reporter.line(&verify(640, 212 * 1024 * 1024)).as_deref(),
            Some("Checking files: 640 / 1,421 (212 MiB / 498 MiB)")
        );
        assert_eq!(reporter.line(&verify(700, 230 * 1024 * 1024)), None);
        assert!(reporter.line(&verify(1421, 498 * 1024 * 1024)).is_some());

        let download = |done| Progress::Download {
            done,
            total: 412 * 1024 * 1024,
        };
        assert_eq!(
            reporter.line(&download(3 * 1024 * 1024)).as_deref(),
            Some("Downloading: 3 MiB / 412 MiB")
        );
        assert_eq!(reporter.line(&download(5 * 1024 * 1024)), None);
        assert!(reporter.line(&download(412 * 1024 * 1024)).is_some());

        let setup = |done, step: Option<&str>| Progress::Setup {
            done,
            total: 2,
            step: step.map(str::to_owned),
        };
        assert_eq!(
            reporter
                .line(&setup(0, Some("script interpreter")))
                .as_deref(),
            Some("Running setup 1 of 2: script interpreter")
        );
        assert_eq!(
            reporter.line(&setup(1, Some("MSVC2017"))).as_deref(),
            Some("Running setup 2 of 2: MSVC2017")
        );
        assert_eq!(reporter.line(&setup(2, None)), None);
    }

    #[test]
    fn setup_rows_say_when_every_step_runs_again() {
        let steps = vec!["script interpreter".to_owned(), "MSVC2017".to_owned()];
        assert_eq!(
            setup_value(&steps, false).as_deref(),
            Some("script interpreter, MSVC2017")
        );
        assert_eq!(
            setup_value(&steps, true).as_deref(),
            Some("script interpreter, MSVC2017, all run again")
        );
        assert_eq!(setup_value(&[], true), None);
        assert_eq!(support_value(0), None);
        assert_eq!(support_value(433).as_deref(), Some("433 files"));
        assert_eq!(support_value(1).as_deref(), Some("1 file"));
    }
}
