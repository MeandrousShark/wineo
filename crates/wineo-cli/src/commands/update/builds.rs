//! `wineo update NAME --builds`: the builds GOG offers for a Galaxy game,
//! read only.

use std::process::ExitCode;

use serde_json::json;
use wineo_core::gog::migrate::MigrateError;
use wineo_core::gog::update::galaxy::listing::{self, Listing};
use wineo_core::paths::Paths;

use super::Args;
use crate::commands::gog_support::{Account, warn};
use crate::commands::table::render_columns;

pub(super) fn run(paths: &Paths, args: &Args) -> anyhow::Result<ExitCode> {
    if paths.portable_root().is_some() {
        return Err(MigrateError::Portable.into());
    }
    // clap requires a name with --builds.
    let name = args.name.as_deref().unwrap_or_default();
    let account = Account::new();
    let mut session = account.session(paths)?;
    let listing = listing::list(paths, &mut session, name, None, args.refresh, false)?;
    for warning in &listing.warnings {
        warn(warning);
    }
    if args.json {
        crate::commands::json::print(&listing_json(&listing))?;
    } else {
        print_table(&listing);
    }
    Ok(ExitCode::SUCCESS)
}

fn listing_json(listing: &Listing) -> serde_json::Value {
    json!({
        "environment": listing.environment,
        "id": listing.gog_id.to_string(),
        "track": "galaxy",
        "branch": listing.follow.branch,
        "pinned": listing.follow.pinned,
        "installed": listing.installed,
        "private_branches": listing.private_branches,
        "builds": listing.builds.iter().map(|build| json!({
            "branch": build.branch,
            "build": build.build,
            "version": build.version,
            "published": build.published,
            "latest": build.latest,
            "private": build.private,
        })).collect::<Vec<_>>(),
    })
}

fn print_table(listing: &Listing) {
    if listing.builds.is_empty() {
        println!(
            "GOG lists no Galaxy builds of {} that wineo can install.",
            listing.title
        );
    } else {
        let mut table = vec![["BRANCH", "BUILD", "VERSION", "PUBLISHED", ""].map(str::to_owned)];
        table.extend(listing.builds.iter().map(|build| {
            [
                build.branch.clone().unwrap_or_else(|| "default".to_owned()),
                build.build.clone(),
                build.version.clone().unwrap_or_else(|| "-".to_owned()),
                build.published_date().unwrap_or("-").to_owned(),
                build.marks().join(", "),
            ]
        }));
        for line in render_columns(&table) {
            println!("{line}");
        }
    }
    if listing.private_branches {
        println!("GOG also has private branches. Name one with --branch NAME.");
    }
}
