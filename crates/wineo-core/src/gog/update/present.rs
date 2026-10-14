//! How updates and installs read, in the words `wineo update` and the app
//! share: product labels, review rows, what changed, and progress lines.
//! Each frontend words its own row labels and writes paths its own way.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use super::install;
use super::{GameError, Pending, Progress, Reason, Updated};
use crate::gog::folder::JournalKind;
use crate::units;

impl Reason {
    /// Why an update can't be applied, or can't be told, as a sentence.
    pub fn note(&self) -> String {
        match self {
            Self::Galaxy => "Updating Galaxy builds comes in a later release.".to_owned(),
            Self::CantCompare(why) => why.clone(),
        }
    }
}

impl GameError {
    /// Names the failed game: its environment, else its id.
    pub fn subject(&self) -> String {
        match (&self.environment, self.id) {
            (Some(name), _) => name.clone(),
            (None, Some(id)) => id.to_string(),
            (None, None) => "A GOG game folder".to_owned(),
        }
    }
}

/// What a row of `Pending::review_rows` is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewLabel {
    Folder,
    /// A product, by its label, which is the row's label as it is.
    Product(String),
    Owner,
    Download,
    Unpacked,
}

impl Pending {
    /// How a review and its result name what is being done, before the
    /// environment's name.
    pub fn action(&self) -> &'static str {
        match (self.kind(), self.is_recovery()) {
            (JournalKind::Reinstall, false) => "Reinstall",
            (JournalKind::Reinstall, true) => "Finish the interrupted reinstall of",
            (JournalKind::Install, true) => "Finish the interrupted install of",
            (_, true) => "Finish the interrupted update of",
            (_, false) => "Update",
        }
    }

    /// What a finished update says it did, without a full stop.
    pub fn finished(&self) -> String {
        let name = self.environment();
        match (self.kind(), self.is_recovery()) {
            (JournalKind::Reinstall, false) => format!("Reinstalled {name}"),
            (JournalKind::Reinstall, true) => {
                format!("Finished the interrupted reinstall of {name}")
            }
            (JournalKind::Install, true) => format!("Finished the interrupted install of {name}"),
            (_, true) => format!("Finished the interrupted update of {name}"),
            (_, false) => format!("Updated {name}"),
        }
    }

    /// The base game's title, which a recovery from saved packages doesn't
    /// have: then the game is named by its environment. A DLC is
    /// `NAME (ID)`.
    pub fn product_label(&self, id: u64, title: Option<&str>) -> String {
        if id != self.target().gog_id {
            return self.dlc_label(id);
        }
        title.map_or_else(|| self.environment().to_owned(), str::to_owned)
    }

    /// Every product's label, by id, for a `Reporter`.
    pub fn product_labels(&self) -> HashMap<u64, String> {
        self.products()
            .iter()
            .map(|product| {
                (
                    product.id,
                    self.product_label(product.id, product.title.as_deref()),
                )
            })
            .collect()
    }

    /// What a review shows, row by row, with `show_path` writing the folder.
    pub fn review_rows(&self, show_path: impl Fn(&Path) -> String) -> Vec<(ReviewLabel, String)> {
        let mut rows = vec![(ReviewLabel::Folder, show_path(self.folder()))];
        for product in self.products() {
            let label = self.product_label(product.id, product.title.as_deref());
            let value = match &product.version {
                Some(version) => change(product.installed.as_deref(), version),
                None => "left as it is".to_owned(),
            };
            rows.push((ReviewLabel::Product(label), value));
        }
        if self.claims() {
            rows.push((
                ReviewLabel::Owner,
                format!("none yet, {} takes this folder", self.environment()),
            ));
        }
        rows.push((
            ReviewLabel::Download,
            if self.is_offline() {
                "none, uses the packages already downloaded".to_owned()
            } else {
                units::size(self.download_size())
            },
        ));
        rows.push((
            ReviewLabel::Unpacked,
            format!(
                "about {} (an estimate)",
                units::size(self.unpacked_estimate())
            ),
        ));
        rows
    }

    /// Each product a finished update installed a version of, by label, with
    /// what changed.
    pub fn changes(&self, updated: &Updated) -> Vec<(String, String)> {
        updated
            .products
            .iter()
            .filter_map(|product| {
                let version = product.version.as_ref()?;
                let planned = self.products().iter().find(|p| p.id == product.id);
                let label =
                    self.product_label(product.id, planned.and_then(|p| p.title.as_deref()));
                let installed = planned.and_then(|p| p.installed.as_deref());
                Some((label, change(installed, version)))
            })
            .collect()
    }
}

impl install::Plan {
    /// The base game's title, or a DLC as `NAME (ID)`.
    pub fn product_label(&self, id: u64) -> String {
        if id == self.gog_id() {
            self.title().to_owned()
        } else {
            self.dlc_label(id)
        }
    }

    /// Every product's label, by id, for a `Reporter`.
    pub fn product_labels(&self) -> HashMap<u64, String> {
        self.products()
            .iter()
            .map(|product| (product.id, self.product_label(product.id)))
            .collect()
    }
}

/// `1.0 -> 1.1`, or the version alone when a rebuild installs the same one.
fn change(installed: Option<&str>, version: &str) -> String {
    match installed {
        Some(installed) if installed == version => format!("{version} (rebuilt)"),
        Some(installed) => format!("{installed} -> {version}"),
        None => format!("unknown -> {version}"),
    }
}

/// Turns progress into status lines: one per stage, and the byte counts
/// at most once per `interval`.
pub struct Reporter {
    labels: HashMap<u64, String>,
    interval: Duration,
    last: Option<Instant>,
    file: Option<(u64, String)>,
    verified: HashSet<u64>,
    unpacking: HashSet<u64>,
    publishing: bool,
}

impl Reporter {
    /// `labels` names each product, by id, the way the review does.
    pub fn new(labels: HashMap<u64, String>, interval: Duration) -> Self {
        Self {
            labels,
            interval,
            last: None,
            file: None,
            verified: HashSet::new(),
            unpacking: HashSet::new(),
            publishing: false,
        }
    }

    fn label(&self, product: u64) -> String {
        self.labels
            .get(&product)
            .cloned()
            .unwrap_or_else(|| format!("GOG product {product}"))
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
            Progress::Download {
                product,
                file_id,
                done,
                total,
            } => {
                let key = (*product, file_id.clone());
                let first = self.file.as_ref() != Some(&key);
                self.file = Some(key);
                (first || done == total || self.due()).then_some(())?;
                let line = format!(
                    "Downloading {}: {} / {}",
                    self.label(*product),
                    units::size(*done),
                    units::size(*total)
                );
                self.said(line)
            }
            Progress::Verify { product } => {
                self.verified.insert(*product).then_some(())?;
                let line = format!("Verifying {}", self.label(*product));
                self.said(line)
            }
            Progress::Stage {
                product,
                done,
                total,
            } => {
                let line = if self.unpacking.insert(*product) {
                    format!("Unpacking {}", self.label(*product))
                } else {
                    self.due().then_some(())?;
                    format!(
                        "Unpacking {}: {} / {}",
                        self.label(*product),
                        units::size(*done),
                        units::size(*total)
                    )
                };
                self.said(line)
            }
            Progress::Publish { done, total } => {
                let line = if !self.publishing {
                    self.publishing = true;
                    format!("Publishing {total} files")
                } else {
                    (done == total || self.due()).then_some(())?;
                    format!("Published {done} of {total} files")
                };
                self.said(line)
            }
            Progress::Installing {
                product,
                interactive,
            } => {
                let line = if *interactive {
                    format!("Opening {}'s setup window", self.label(*product))
                } else {
                    format!("Running {}'s installer", self.label(*product))
                };
                self.said(line)
            }
        }
    }
}
