//! Galaxy builds in `wineo import gog --game` and `wineo update`: the rows
//! of their reviews, progress lines, and what a finished run says. The
//! wording lives here, apart from the flow that uses it.

use std::time::Duration;

use wineo_core::cancel::Token;
use wineo_core::gog::folder::Track;
use wineo_core::gog::update::galaxy::present::{Label, saved_items};
pub use wineo_core::gog::update::galaxy::present::{Reporter, restart_lines};
use wineo_core::gog::update::galaxy::update::Plan;
use wineo_core::gog::update::galaxy::{FollowPlan, Progress};
use wineo_core::gog::update::{SavedAside, Updated};
use wineo_core::paths::Paths;

use super::table;

/// What `--track` takes.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
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

/// A row's label as the CLI's table words it.
fn word(label: Label) -> String {
    match label {
        Label::Folder => "folder".to_owned(),
        Label::Track => "track".to_owned(),
        Label::Branch => "branch".to_owned(),
        Label::Pin => "pin".to_owned(),
        Label::Owner => "owner".to_owned(),
        Label::Product(label) => label,
        Label::Version => "version".to_owned(),
        Label::Dlc => "dlc".to_owned(),
        Label::Support => "support".to_owned(),
        Label::Check => "check".to_owned(),
        Label::Setup => "setup".to_owned(),
        Label::Download => "download".to_owned(),
    }
}

/// The lines of an update's review: its title, the rows, and the notes that
/// follow.
pub fn update_review(paths: &Paths, plan: &Plan) -> Vec<String> {
    let mut lines = vec![format!("{} {}", plan.action(), plan.environment())];
    let rows = plan
        .review_rows(paths, |path| crate::style::path(paths, path))
        .into_iter()
        .map(|(label, value)| (word(label), value))
        .collect();
    lines.extend(table::aligned_rows(rows));
    lines.extend(plan.note_lines().into_iter().map(str::to_owned));
    lines
}

/// The lines of a branch or pin change's review.
pub fn follow_review(paths: &Paths, plan: &FollowPlan) -> Vec<String> {
    let mut lines = vec![format!("{} {}", plan.action(), plan.environment())];
    let rows = plan
        .review_rows(|path| crate::style::path(paths, path))
        .into_iter()
        .map(|(label, value)| (word(label), value))
        .collect();
    lines.extend(table::aligned_rows(rows));
    lines
}

/// A Galaxy install's rows after its platform, language, and runner: the
/// build, DLC, support files, setup, and size.
pub fn install_rows(
    paths: &Paths,
    plan: &wineo_core::gog::update::galaxy::install::Plan,
    rows: &mut Vec<(String, String)>,
) {
    // One DLC per line; the rest sit under the first one's label.
    let mut dlc = false;
    for (label, value) in plan.review_rows(paths) {
        let label = match label {
            Label::Dlc if std::mem::replace(&mut dlc, true) => String::new(),
            label => word(label),
        };
        rows.push((label, value));
    }
}

/// The rows an update reports as changed: the products it installed, as the
/// review showed them.
pub fn changes(paths: &Paths, plan: &Plan, updated: &Updated) -> Vec<(String, String)> {
    plan.changes(paths, updated)
}

/// What a finished run says about files it moved aside.
pub fn saved_lines(plan: &Plan, saved: &[SavedAside]) -> Vec<String> {
    if saved.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![plan.saved_heading().to_owned()];
    lines.extend(
        saved_items(saved)
            .into_iter()
            .map(|item| format!("  {item}")),
    );
    lines
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
