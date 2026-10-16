//! How Galaxy installs and updates read, in the words `wineo` and the app
//! share: review rows, notes, what changed, and progress lines. Each
//! frontend words its own row labels and writes paths its own way.

use std::path::Path;
use std::time::{Duration, Instant};

use super::Progress;
use super::install;
use super::record::Follow;
use super::switch::FollowPlan;
use super::update::{Note, Plan};
use crate::gog::folder::{JournalKind, ProductStatus};
use crate::gog::update::{SavedAside, Updated};
use crate::paths::Paths;
use crate::units;

const SPACE_UNCOUNTED: &str = "Setup's own writes in the prefix aren't counted in the space check.";
const NOTHING_DELETED_MOVE: &str =
    "Nothing is deleted: files the installer left that this build doesn't list stay.";
const NOTHING_DELETED: &str =
    "Nothing is deleted: files of the installed build that this one doesn't list stay.";

/// What a row of a Galaxy review is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Label {
    Folder,
    Track,
    /// The branch followed, or the change of it.
    Branch,
    /// A pin to a build, or its removal.
    Pin,
    Owner,
    /// A product of an update, by its label, which is the row's label as it
    /// is.
    Product(String),
    /// The base game of an install.
    Version,
    /// A DLC of an install. Every one has its own row.
    Dlc,
    Support,
    Check,
    Setup,
    Download,
}

/// Who a review is worded for. Only the pin row differs: the CLI's says
/// which flag undoes it, which means nothing in the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wording {
    Cli,
    App,
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

fn files_word(count: usize) -> &'static str {
    if count == 1 { "file" } else { "files" }
}

/// A build as a person reads it: GOG's name for it, else its id.
pub fn version(name: Option<&str>, build: &str) -> String {
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

/// The support files as a row's value, when there are any.
pub fn support_value(files: usize) -> Option<String> {
    (files > 0).then(|| format!("{} {}", thousands(files), files_word(files)))
}

/// The paths a finished run moved aside, as `PATH -> SAVED_AS`.
pub fn saved_items(saved: &[SavedAside]) -> Vec<String> {
    saved
        .iter()
        .map(|saved| format!("{} -> {}", saved.path, saved.saved_as))
        .collect()
}

/// One line for each redistributable that asked for a restart.
pub fn restart_lines(needs_restart: &[String]) -> Vec<String> {
    needs_restart
        .iter()
        .map(|step| format!("{step} asked for a restart, which Wine doesn't need."))
        .collect()
}

/// The branch and pin rows of a review. With `from`, the game follows
/// something already, and a row shows what changes; without, it's an
/// install, and a row shows what it will follow.
pub fn follow_rows(from: Option<&Follow>, to: &Follow, wording: Wording) -> Vec<(Label, String)> {
    let branch = |branch: &Option<String>| branch.as_deref().unwrap_or("default").to_owned();
    let mut rows = Vec::new();
    match from {
        Some(from) if from.branch != to.branch => rows.push((
            Label::Branch,
            format!("{} -> {}", branch(&from.branch), branch(&to.branch)),
        )),
        _ => {
            if let Some(name) = &to.branch {
                rows.push((Label::Branch, name.clone()));
            }
        }
    }
    if from.is_none_or(|from| from.pinned != to.pinned) {
        match &to.pinned {
            Some(build) => rows.push((
                Label::Pin,
                match wording {
                    Wording::Cli => format!("build {build}, until --latest"),
                    Wording::App => format!("build {build}"),
                },
            )),
            None if from.is_some() => rows.push((Label::Pin, "removed".to_owned())),
            None => {}
        }
    }
    rows
}

impl FollowPlan {
    /// How a review names the change, before the environment's name.
    pub fn action(&self) -> &'static str {
        if self.to().pinned.is_some() && self.to().pinned != self.from().pinned {
            "Pin"
        } else if self.to().branch != self.from().branch {
            "Switch the branch of"
        } else {
            "Unpin"
        }
    }

    /// What a finished change says it did, without a full stop.
    pub fn finished(&self) -> String {
        let name = self.environment();
        let to = self.to();
        if let (Some(build), true) = (&to.pinned, to.pinned != self.from().pinned) {
            return format!("Pinned {name} to build {build}");
        }
        match (&to.branch, to.branch != self.from().branch) {
            (None, true) => format!("{name} now follows the default branch"),
            (Some(branch), true) => format!("{name} now follows branch {branch}"),
            _ => format!("{name} follows its branch again"),
        }
    }

    /// The rows of the review: nothing but the folder, track, and what
    /// changes about the branch and pin.
    pub fn review_rows(
        &self,
        show_path: impl Fn(&Path) -> String,
        wording: Wording,
    ) -> Vec<(Label, String)> {
        let mut rows = vec![
            (Label::Folder, show_path(self.folder())),
            (Label::Track, "Galaxy".to_owned()),
        ];
        rows.extend(follow_rows(Some(self.from()), self.to(), wording));
        if self.claims() {
            rows.push((
                Label::Owner,
                format!("none yet, {} takes this folder", self.environment()),
            ));
        }
        rows
    }
}

impl Plan {
    /// What an update, repair, reinstall, or move reviews, row by row, with
    /// `show_path` writing the folder.
    pub fn review_rows(
        &self,
        paths: &Paths,
        show_path: impl Fn(&Path) -> String,
        wording: Wording,
    ) -> Vec<(Label, String)> {
        let mut rows = vec![(Label::Folder, show_path(self.folder()))];
        let track = if self.moves_from().is_some() {
            "installer -> Galaxy"
        } else {
            "Galaxy"
        };
        rows.push((Label::Track, track.to_owned()));
        rows.extend(follow_rows(Some(self.from()), self.to(), wording));
        if self.claims() {
            rows.push((
                Label::Owner,
                format!("none yet, {} takes this folder", self.environment()),
            ));
        }
        let labels = self.product_labels();
        let target = version(self.version_name(), self.build());
        for product in self.products() {
            let label = labels
                .get(&product.id)
                .cloned()
                .unwrap_or_else(|| product.id.to_string());
            let value = if product.status != ProductStatus::Installed {
                "unavailable".to_owned()
            } else if product.id != self.gog_id() {
                target.clone()
            } else {
                self.base_change(&target)
            };
            rows.push((Label::Product(label), value));
        }
        if let Some(files) = support_value(self.support_files()) {
            rows.push((Label::Support, files));
        }
        if self.kind() == JournalKind::Repair {
            let (files, bytes) = self.checked();
            rows.push((
                Label::Check,
                format!(
                    "every file, {} {} ({})",
                    thousands(files),
                    files_word(files),
                    units::size(bytes)
                ),
            ));
        }
        let steps = self.setup_steps(paths);
        if let Some(value) = setup_value(&steps, self.hashes_all()) {
            rows.push((Label::Setup, value));
        }
        let download = match self.kind() {
            JournalKind::Repair => "only what's missing or damaged".to_owned(),
            JournalKind::Reinstall => format!(
                "up to {}, files that already match are kept",
                units::size(self.download_size())
            ),
            _ => units::size(self.download_size()),
        };
        rows.push((Label::Download, download));
        rows
    }

    /// The base game's row: where it was and where it goes.
    fn base_change(&self, target: &str) -> String {
        if self.kind() == JournalKind::Repair {
            return target.to_owned();
        }
        let moving = self.moves_from().is_some();
        let from = if moving {
            self.installed_version().map(str::to_owned)
        } else {
            self.installed_build()
                .map(|build| version(self.installed_version(), build))
        };
        let Some(from) = from else {
            return format!("unknown -> {target}");
        };
        if !moving && self.installed_build() == Some(self.build()) {
            return if self.kind() == JournalKind::Reinstall {
                format!("{target} (rebuilt)")
            } else {
                target.to_owned()
            };
        }
        if from == target {
            // The same name on another build, as when an installer's version
            // moves onto Galaxy.
            return format!("{from} -> {target} (build {})", self.build());
        }
        format!("{from} -> {target}")
    }

    /// The notes a review gives after its rows.
    pub fn note_lines(&self) -> Vec<&'static str> {
        self.notes()
            .iter()
            .map(|note| match note {
                Note::NothingDeleted if self.moves_from().is_some() => NOTHING_DELETED_MOVE,
                Note::NothingDeleted => NOTHING_DELETED,
                Note::SetupSpaceUncounted => SPACE_UNCOUNTED,
            })
            .collect()
    }

    /// The rows an update reports as changed: the products it installed, as
    /// the review showed them.
    pub fn changes(&self, paths: &Paths, updated: &Updated) -> Vec<(String, String)> {
        // Only the product rows are read, which every wording shares.
        let rows = self.review_rows(paths, |path| path.display().to_string(), Wording::Cli);
        let labels = self.product_labels();
        updated
            .products
            .iter()
            .filter(|product| product.status == ProductStatus::Installed)
            .filter_map(|product| {
                let label = labels.get(&product.id)?;
                rows.iter().find_map(|(row, value)| match row {
                    Label::Product(name) if name == label => Some((name.clone(), value.clone())),
                    _ => None,
                })
            })
            .collect()
    }

    /// What a finished run says before its list of files moved aside.
    pub fn saved_heading(&self) -> &'static str {
        if matches!(self.kind(), JournalKind::Repair | JournalKind::Reinstall) {
            "Saved these files aside, since they didn't match GOG's:"
        } else {
            "Moved your files aside where the new version has files:"
        }
    }
}

impl install::Plan {
    /// A Galaxy install's rows after its platform, language, and runner: the
    /// build, DLC, support files, setup, and size.
    pub fn review_rows(&self, paths: &Paths, wording: Wording) -> Vec<(Label, String)> {
        let mut rows = Vec::new();
        let target = version(self.version_name(), self.build());
        let labels = self.product_labels();
        rows.extend(follow_rows(None, self.follow(), wording));
        for product in self.products() {
            let value = if product.status == ProductStatus::Installed {
                target.clone()
            } else {
                "unavailable".to_owned()
            };
            if product.id == self.gog_id() {
                rows.push((Label::Version, value));
            } else if let Some(label) = labels.get(&product.id) {
                rows.push((Label::Dlc, format!("{label} {value}")));
            }
        }
        if let Some(files) = support_value(self.support_files()) {
            rows.push((Label::Support, files));
        }
        if let Some(value) = setup_value(&self.setup_steps(paths), false) {
            rows.push((Label::Setup, value));
        }
        rows.push((Label::Download, units::size(self.installed_size())));
        rows
    }
}

/// Turns Galaxy progress into status lines: one per stage, and the byte
/// counts at most once per `interval`.
pub struct Reporter {
    interval: Duration,
    last: Option<Instant>,
    checking: bool,
    /// The last download line said, which the end of one doesn't repeat.
    downloaded: Option<(u64, u64)>,
}

impl Reporter {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
            checking: false,
            downloaded: None,
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
                let first = self.downloaded.is_none();
                (first || done == total || self.due()).then_some(())?;
                (self.downloaded.replace((*done, *total)) != Some((*done, *total))).then_some(())?;
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

    #[test]
    fn restart_lines_say_wine_needs_no_restart() {
        assert_eq!(
            restart_lines(&["MSVC2017".to_owned()]),
            ["MSVC2017 asked for a restart, which Wine doesn't need."]
        );
        assert!(restart_lines(&[]).is_empty());
    }

    #[test]
    fn a_build_reads_by_its_name_else_its_id() {
        assert_eq!(version(Some("1.2.76"), "100"), "1.2.76");
        assert_eq!(version(Some(""), "100"), "100");
        assert_eq!(version(None, "100"), "100");
    }

    #[test]
    fn only_the_cli_names_the_flag_that_undoes_a_pin() {
        let from = Follow::default();
        let to = Follow {
            branch: None,
            pinned: Some("100".to_owned()),
        };
        assert_eq!(
            follow_rows(Some(&from), &to, Wording::Cli),
            [(Label::Pin, "build 100, until --latest".to_owned())]
        );
        assert_eq!(
            follow_rows(Some(&from), &to, Wording::App),
            [(Label::Pin, "build 100".to_owned())]
        );
        // Unpinning reads the same in both.
        for wording in [Wording::Cli, Wording::App] {
            assert_eq!(
                follow_rows(Some(&to), &from, wording),
                [(Label::Pin, "removed".to_owned())]
            );
        }
    }
}
