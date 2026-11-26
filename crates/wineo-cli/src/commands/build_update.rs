//! `wineo runners update`: newer builds of what wineo downloaded, found on
//! GitHub and fetched beside the old ones.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::builder::{PossibleValuesParser, TypedValueParser};
use serde::Serialize;
use wineo_core::build_lock::Kind;
use wineo_core::build_removal;
use wineo_core::build_update::{self, NotInstalled, Outcome, Report, Status};
use wineo_core::config::GlobalConfig;
use wineo_core::download::{Client, Progress, Provider};
use wineo_core::paths::Paths;

use super::transfer::Cancellation;
use crate::{cancel, style, text};

#[derive(clap::Args)]
pub struct Args {
    /// ge-proton, proton-cachyos, dxvk, or vkd3d; omit for every one that has a build
    #[arg(value_name = "FAMILY", value_parser = family_parser())]
    families: Vec<Provider>,
    #[command(flatten)]
    mode: Mode,
}

/// `wineo dxvk update` and `wineo vkd3d update`, for the one family their
/// command is about.
#[derive(clap::Args)]
pub struct FamilyArgs {
    #[command(flatten)]
    mode: Mode,
}

#[derive(clap::Args, Clone, Copy)]
struct Mode {
    /// Print what is newer and download nothing
    #[arg(long)]
    check: bool,
    /// With --check, print JSON
    #[arg(long, requires = "check")]
    json: bool,
}

fn family_parser() -> impl TypedValueParser<Value = Provider> {
    PossibleValuesParser::new(Provider::ALL.map(Provider::id))
        .map(|id| id.parse().expect("only a possible value gets here"))
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    update(paths, &args.families, args.mode)
}

pub fn run_family(paths: &Paths, provider: Provider, args: FamilyArgs) -> anyhow::Result<ExitCode> {
    update(paths, &[provider], args.mode)
}

fn update(paths: &Paths, families: &[Provider], mode: Mode) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let cancellation = Cancellation::new()?;
    let client = Client::new()?;
    let report = build_update::check(&client, paths, &global, families, &cancellation.token);
    if cancellation.token.is_canceled() {
        return Ok(canceled());
    }
    if mode.check && mode.json {
        super::json::print(&listing(&report))?;
        return Ok(exit_code(report.failed()));
    }

    // A check is a query, so its table is the output. A run is progress, so
    // everything it says goes to stderr and stdout stays empty.
    let say = |line: &str| {
        if mode.check {
            println!("{line}");
        } else {
            eprintln!("{line}");
        }
    };
    if report.families.is_empty() {
        say("Nothing to update.");
        return Ok(ExitCode::SUCCESS);
    }
    for line in table(&report) {
        say(&line);
    }
    let mut failures = check_failures(&report);
    for (_, message) in &failures {
        eprintln!("{}", style::error_stderr(format!("error: {message}")));
    }
    if mode.check {
        if !failures.is_empty() {
            eprintln!("{}", summary("check", &failures, &report));
        }
        return Ok(exit_code(!failures.is_empty()));
    }

    let applied = download(&client, paths, &global, &report, &cancellation);
    let releases: Vec<_> = report.updates().collect();
    let stopped = cancellation.token.is_canceled();
    let mut installed = Vec::new();
    let mut already = Vec::new();
    let mut updated = Vec::new();
    for (release, (provider, outcome)) in releases.iter().zip(&applied.families) {
        match outcome {
            Outcome::Installed(_) => {
                installed.push(release.installed_name.as_str());
                updated.push(*provider);
            }
            Outcome::AlreadyThere(_) => {
                already.push(release.installed_name.as_str());
                updated.push(*provider);
            }
            // A canceled batch says so once, not once per family it skipped.
            Outcome::Failed(error) if stopped && cancel::core(error) => {}
            Outcome::Failed(error) => {
                let message = format!("Couldn't download {}: {error:#}", release.installed_name);
                eprintln!("{}", style::error_stderr(format!("error: {message}")));
                failures.push((provider.label(), message));
            }
        }
    }
    if !installed.is_empty() {
        eprintln!("Installed {}.", join(&installed, "and"));
    }
    if !already.is_empty() {
        eprintln!("Already installed: {}.", join(&already, "and"));
    }
    if stopped {
        return Ok(canceled());
    }
    if let Some(line) = older_builds_line(paths, &global, &updated) {
        eprintln!("{line}");
    }
    if !failures.is_empty() {
        eprintln!("{}", summary("update", &failures, &report));
    }
    Ok(exit_code(!failures.is_empty()))
}

/// Downloads what the check found, printing progress as `wineo runners download`
/// does, with the name of each build as its download starts.
fn download(
    client: &Client,
    paths: &Paths,
    global: &GlobalConfig,
    report: &Report,
    cancellation: &Cancellation,
) -> build_update::Applied {
    let mut current = None;
    let mut last = Instant::now() - Duration::from_secs(1);
    build_update::apply(client, paths, global, report, &cancellation.token, |step| {
        if matches!(
            step.progress,
            Progress::Downloading { .. } | Progress::Verifying | Progress::Extracting
        ) && current != Some(step.index)
        {
            current = Some(step.index);
            eprintln!("Downloading {}", step.release.installed_name);
        }
        match step.progress {
            Progress::Downloading { received, total }
                if last.elapsed() >= Duration::from_secs(1) =>
            {
                eprintln!(
                    "Downloaded {} / {}",
                    style::size(received),
                    style::size(total)
                );
                last = Instant::now();
            }
            Progress::Verifying => eprintln!("Verifying download"),
            Progress::Extracting => eprintln!("Extracting build"),
            _ => {}
        }
    })
}

fn canceled() -> ExitCode {
    eprintln!("{}", style::warning_stderr("Update canceled"));
    cancel::exit_code()
}

fn exit_code(failed: bool) -> ExitCode {
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// One line per family that has a build: what it is now and what is newer.
/// A family that couldn't be checked is reported on its own.
fn table(report: &Report) -> Vec<String> {
    let rows: Vec<(&str, String)> = report
        .families
        .iter()
        .filter_map(|family| {
            let installed = family.installed.as_deref()?;
            let detail = match &family.status {
                Status::UpToDate => format!("up to date ({installed})"),
                Status::Available(release) => format!(
                    "{installed} -> {} ({})",
                    release.installed_name,
                    style::size(release.size)
                ),
                Status::Failed(_) => return None,
            };
            Some((family.provider.label(), detail))
        })
        .collect();
    let width = text::widest(rows.iter().map(|(label, _)| *label));
    rows.iter()
        .map(|(label, detail)| format!("{}  {detail}", text::pad(label, width)))
        .collect()
}

/// Each family that couldn't be checked, with what went wrong, in order.
fn check_failures(report: &Report) -> Vec<(&'static str, String)> {
    report
        .families
        .iter()
        .filter_map(|family| match &family.status {
            Status::Failed(error) => Some((
                family.provider.label(),
                failure_message(family.provider, error),
            )),
            _ => None,
        })
        .collect()
}

fn failure_message(provider: Provider, error: &anyhow::Error) -> String {
    if error.is::<NotInstalled>() {
        // The core knows a family isn't installed; the command knows how to get one.
        format!(
            "{error}. Download it with wineo {}.",
            super::download::invocation(provider)
        )
    } else {
        format!("Couldn't check {}: {error:#}", provider.label())
    }
}

/// The closing line after one or more families failed.
fn summary(verb: &str, failures: &[(&str, String)], report: &Report) -> String {
    let total = report.families.len();
    let names: Vec<&str> = failures.iter().map(|(label, _)| *label).collect();
    style::error_stderr(format!(
        "Couldn't {verb} {} of {total} {}: {}.",
        failures.len(),
        text::plural(total, "family", "families"),
        names.join(", ")
    ))
}

#[derive(Serialize)]
struct Listing {
    families: Vec<FamilyJson>,
    errors: Vec<ErrorJson>,
}

#[derive(Serialize)]
struct FamilyJson {
    provider: &'static str,
    installed: String,
    available: Option<String>,
    update: &'static str,
    download_size: Option<u64>,
}

#[derive(Serialize)]
struct ErrorJson {
    provider: &'static str,
    message: String,
}

fn listing(report: &Report) -> Listing {
    let mut listing = Listing {
        families: Vec::new(),
        errors: Vec::new(),
    };
    for family in &report.families {
        let provider = family.provider.id();
        let installed = family.installed.clone().unwrap_or_default();
        match &family.status {
            Status::UpToDate => listing.families.push(FamilyJson {
                provider,
                installed,
                available: None,
                update: "none",
                download_size: None,
            }),
            Status::Available(release) => listing.families.push(FamilyJson {
                provider,
                installed,
                available: Some(release.installed_name.clone()),
                update: "available",
                download_size: Some(release.size),
            }),
            Status::Failed(error) => listing.errors.push(ErrorJson {
                provider,
                message: failure_message(family.provider, error),
            }),
        }
    }
    listing
}

/// Names the older builds of the families just updated that nothing uses,
/// with the commands that remove them. Left out when none are, and when a
/// config couldn't be read, since then nothing can be called unused and
/// `remove` would refuse.
fn older_builds_line(paths: &Paths, global: &GlobalConfig, updated: &[Provider]) -> Option<String> {
    let mut candidates: Vec<(Provider, String)> = Vec::new();
    for provider in updated {
        for name in build_update::older_builds(paths, global, *provider).ok()? {
            candidates.push((*provider, name));
        }
    }
    if candidates.is_empty() {
        return None;
    }
    let uses = build_removal::uses_all(paths).ok()?;
    if uses.values().any(|uses| !uses.errors.is_empty()) {
        return None;
    }
    let unused: Vec<(Provider, &str)> = candidates
        .iter()
        .filter(|(provider, name)| {
            uses.get(&(kind(*provider), name.clone()))
                .is_some_and(|uses| uses.labels.is_empty())
        })
        .map(|(provider, name)| (*provider, name.as_str()))
        .collect();
    if unused.is_empty() {
        return None;
    }
    let names: Vec<&str> = unused.iter().map(|(_, name)| *name).collect();
    let mut commands: Vec<String> = Vec::new();
    for (provider, _) in &unused {
        let command = format!("`wineo {} remove NAME`", remover(*provider));
        if !commands.contains(&command) {
            commands.push(command);
        }
    }
    let commands: Vec<&str> = commands.iter().map(String::as_str).collect();
    Some(format!(
        "Nothing uses {} now. Remove {} with {}.",
        join(&names, "or"),
        text::plural(names.len(), "it", "them"),
        join(&commands, "and"),
    ))
}

fn kind(provider: Provider) -> Kind {
    match provider {
        Provider::GeProton | Provider::ProtonCachyos => Kind::Runner,
        Provider::Dxvk => Kind::Dxvk,
        Provider::Vkd3d => Kind::Vkd3d,
    }
}

/// The command word that lists and removes a family's builds.
fn remover(provider: Provider) -> &'static str {
    match provider {
        Provider::GeProton | Provider::ProtonCachyos => "runners",
        Provider::Dxvk => "dxvk",
        Provider::Vkd3d => "vkd3d",
    }
}

/// `a`, `a and b`, or `a, b, and c`.
fn join(items: &[&str], word: &str) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [first, second] => format!("{first} {word} {second}"),
        [rest @ .., last] => format!("{}, {word} {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_join_the_way_a_sentence_does() {
        assert_eq!(join(&["a"], "and"), "a");
        assert_eq!(join(&["a", "b"], "or"), "a or b");
        assert_eq!(join(&["a", "b", "c"], "and"), "a, b, and c");
    }
}
