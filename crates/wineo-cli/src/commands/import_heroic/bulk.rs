//! The `--all` bulk import: the plan table and the per-game loop.

use std::io::Write;
use std::process::ExitCode;
use wineo_core::components::Component;
use wineo_core::config::GlobalConfig;
use wineo_core::download::Provider;
use wineo_core::heroic::{HeroicGameKind, HeroicSource};
use wineo_core::heroic_import::{
    Entry, Game, GamePlan, LibraryOptions, LibraryPlan, SkipReason, listing, plan_library,
};
use wineo_core::paths::Paths;
use wineo_core::runners::Runners;
use wineo_core::steamgriddb::Key;

use super::summary::{show_game_summary, warnings_line};
use super::{
    Args, ArtChoice, apply_plan_with_cancellation, dry_run, fetch_art, finish_registered, native,
    print_log_path, report_playtime, source,
};
use crate::cancel;
use crate::commands::art::{Access, KeyCheck, Pick};
use crate::commands::components;
use crate::commands::download;
use crate::commands::transfer::Cancellation;
use crate::prompt::{self, Canceled, Input, NoInput, Prompt, Reply, Terminal};
use crate::style;
use crate::text;

/// A candidate as `--all` displays it before importing: how it's named, and
/// what happens to it.
pub(super) struct BulkRow {
    pub(super) name: String,
    pub(super) title: String,
    pub(super) status: RowStatus,
    pub(super) launch_scripts: bool,
}

pub(super) enum RowStatus {
    Ready {
        warn: Option<String>,
    },
    /// A native game, which has no prefix to copy or adopt.
    ReadyNative {
        warn: Option<String>,
    },
    Skipped(String),
}

pub(super) fn run_all(
    paths: &Paths,
    global: &GlobalConfig,
    access: &Access,
    args: Args,
    verbose: bool,
) -> anyhow::Result<ExitCode> {
    anyhow::ensure!(
        args.dry_run || args.yes || prompt::interactive(),
        "bulk Heroic import needs a terminal; pass --yes to skip the confirmation"
    );
    let source = source(paths, args.config_dir.as_deref())?;
    let runners = Runners::new(paths, global)?;
    let mut prompt = if args.yes || args.dry_run {
        None
    } else {
        Some(
            Prompt::new(Terminal::open()?, crate::output::stdout())
                .with_color(style::stdout_enabled())
                .with_first_question(true),
        )
    };

    let mut listing = listing(paths, global, &source)?;
    // Native games need no runner, so a library of only those never asks for one.
    let has_windows = listing
        .candidates
        .iter()
        .any(|candidate| candidate.kind != HeroicGameKind::Native);
    let candidate_count = listing.candidates.len();
    // Printed after the table, where it explains why fewer games are listed
    // than were checked.
    let mut hidden_note = None;
    let runner = if !has_windows && args.runner.is_none() {
        None
    } else {
        match &args.runner {
            Some(name) => {
                runners.resolve(name)?;
                Some(name.clone())
            }
            None => match runners.newest_proton()? {
                Some(runner) => Some(runner.selection().to_owned()),
                None if args.dry_run && !args.yes => {
                    println!("Would offer to download the latest GE-Proton.");
                    let count = listing
                        .candidates
                        .iter()
                        .filter(|candidate| candidate.kind != HeroicGameKind::Native)
                        .count();
                    let (noun, need, target) = if count == 1 {
                        ("game", "it needs", "it")
                    } else {
                        ("games", "they need", "them")
                    };
                    hidden_note = Some(format!(
                        "{count} Windows {noun} not shown: {need} a Proton runner, which wineo would offer to download. Pass --runner to preview {target}."
                    ));
                    listing
                        .candidates
                        .retain(|candidate| candidate.kind == HeroicGameKind::Native);
                    None
                }
                None => {
                    anyhow::ensure!(
                        !args.yes,
                        "no Proton runner is installed; pass --runner or download one first"
                    );
                    let prompt = prompt.as_mut().expect("interactive was checked above");
                    Some(download_latest_ge_proton(paths, global, &runners, prompt)?)
                }
            },
        }
    };

    println!("Checking {candidate_count} Heroic games...");
    let mut library = plan_library(
        paths,
        global,
        &listing,
        runner.as_deref(),
        &LibraryOptions {
            adopt: args.adopt,
            arch: args.arch,
        },
    )?;
    if !args.dry_run {
        library.record_backfills(paths)?;
        resolve_pending(paths, global, &mut library, args.yes, prompt.as_mut())?;
    } else if args.yes {
        library.settle(paths, global);
    }
    let runner_name = library.runner().unwrap_or_default();

    let mode_label = if args.adopt { "adopt" } else { "copy" };
    println!(
        "{}",
        render_bulk_table(
            runner_name,
            mode_label,
            &bulk_rows(&library),
            library.already()
        )
    );
    if let Some(note) = hidden_note {
        println!("{note}");
    }
    if args.dry_run {
        let failed = dry_run::library(paths, global, access, &library, args.art, args.yes)?;
        println!("Dry run, nothing was imported.");
        return Ok(if failed {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        });
    }
    if library.ready_count() == 0 {
        println!("No Heroic games are ready to import.");
        return Ok(ExitCode::SUCCESS);
    }
    let portable = paths.portable_root().is_some();
    let desktop_enabled = match bulk_desktop_choice(args.desktop, args.yes || portable) {
        Some(value) => value,
        None => {
            let prompt = prompt.as_mut().expect("interactive was checked above");
            match prompt.confirm("Create desktop entries for these games?", true)? {
                Reply::Value(value) => value,
                Reply::Back => return Err(Canceled.into()),
            }
        }
    };

    let art = if global.art.fetch.kinds().is_empty() {
        None
    } else {
        bulk_art(access, args.art, args.yes, prompt.as_mut())?
    };

    if !args.yes {
        let prompt = prompt.as_mut().expect("interactive was checked above");
        let count = library.ready_count();
        match prompt.confirm(&format!("Import {count} games?"), true)? {
            Reply::Value(true) => {}
            _ => return Err(Canceled.into()),
        }
    }

    let bulk = Bulk {
        paths,
        global,
        access,
        source: &source,
        desktop: desktop_enabled,
        art,
        verbose,
    };
    import_all(&bulk, library.into_ready())
}

/// The key `--all` fetches art with, or None for no art. `--art` answers yes
/// ahead of time and `--yes` without it answers no, as with desktop entries.
/// A missing key is only noted when art was asked for or would be asked about.
pub(super) fn bulk_art<I: Input, W: Write>(
    access: &Access,
    art_flag: bool,
    yes_flag: bool,
    mut prompt: Option<&mut Prompt<I, W>>,
) -> anyhow::Result<Option<Key>> {
    if yes_flag && !art_flag {
        return Ok(None);
    }
    let key = match (access.check_key)() {
        KeyCheck::Found(key) => key,
        check => {
            match (prompt.as_mut(), check.skip_line()) {
                (Some(prompt), Some(line)) => prompt.say(line)?,
                _ => check.print_skip(),
            }
            return Ok(None);
        }
    };
    if art_flag {
        return Ok(Some(key));
    }
    let prompt = prompt.expect("interactive was checked above");
    match prompt.confirm("Fetch art for these games?", true)? {
        Reply::Value(wanted) => Ok(wanted.then_some(key)),
        Reply::Back => Err(Canceled.into()),
    }
}

fn download_latest_ge_proton<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    runners: &Runners,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<String> {
    match prompt.confirm(
        "No Proton runner is installed. Download the latest GE-Proton?",
        true,
    )? {
        Reply::Value(true) => {}
        _ => return Err(Canceled.into()),
    }
    let name = download::install_latest(paths, global, Provider::GeProton)?;
    runners.resolve(&name)?;
    Ok(name)
}

pub(super) fn resolve_pending<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    library: &mut LibraryPlan,
    yes: bool,
    prompt: Option<&mut Prompt<I, W>>,
) -> anyhow::Result<()> {
    resolve_pending_with(paths, global, library, yes, prompt, |component, version| {
        components::download_build(paths, global, component, version)
    })
}

pub(super) fn resolve_pending_with<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    library: &mut LibraryPlan,
    yes: bool,
    mut prompt: Option<&mut Prompt<I, W>>,
    mut download: impl FnMut(Component, Option<&str>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut offered = Vec::new();

    while !yes && let Some(prompt) = prompt.as_mut() {
        let Some(offer) = library
            .pending_builds()
            .into_iter()
            .find(|offer| !offered.contains(&offer.component))
        else {
            break;
        };
        let (component, count, unique) = (offer.component, offer.games, offer.versions);
        offered.push(component);
        let pinned: Vec<&str> = unique.iter().filter_map(|v| v.as_deref()).collect();
        let question = if pinned.is_empty() {
            format!(
                "No matching {} build is installed for {count} game{}. Download the latest release?",
                component.label(),
                text::plural(count, "", "s")
            )
        } else {
            format!(
                "{} is missing for {count} game{} ({}). Download the exact requested version{}?",
                component.label(),
                text::plural(count, "", "s"),
                pinned.join(", "),
                text::plural(pinned.len(), "", "s")
            )
        };
        if !matches!(prompt.confirm(&question, true)?, Reply::Value(true)) {
            continue;
        }
        for version in &unique {
            if version.is_none() && !pinned.is_empty() {
                continue;
            }
            download(component, version.as_deref())?;
        }
        library.recheck(paths, global);
    }

    library.settle(paths, global);
    Ok(())
}

/// The bulk table's rows, in the order the plan lists its games.
pub(super) fn bulk_rows(library: &LibraryPlan) -> Vec<BulkRow> {
    library
        .entries()
        .iter()
        .map(|entry| match entry {
            Entry::Ready(Game::Windows(plan)) => BulkRow {
                name: plan.name().to_owned(),
                title: plan.heroic().title.clone(),
                status: RowStatus::Ready {
                    warn: warnings_line(&plan.warnings(), plan.is_adopt()),
                },
                launch_scripts: !plan.heroic().settings_review.hooks().is_empty(),
            },
            Entry::Ready(Game::Native(plan)) => BulkRow {
                name: plan.name().to_owned(),
                title: plan.heroic().title.clone(),
                status: RowStatus::ReadyNative {
                    warn: warnings_line(&plan.warnings(), false),
                },
                launch_scripts: !plan.heroic().settings_review.hooks().is_empty(),
            },
            Entry::Pending(game) => BulkRow {
                name: game.plan().name().to_owned(),
                title: game.plan().heroic().title.clone(),
                status: RowStatus::Skipped(game.reason().to_owned()),
                launch_scripts: false,
            },
            Entry::Skipped(skipped) => BulkRow {
                name: skipped.name.clone(),
                title: skipped.title.clone(),
                status: RowStatus::Skipped(skip_text(&skipped.reason)),
                launch_scripts: false,
            },
        })
        .collect()
}

fn skip_text(reason: &SkipReason) -> String {
    match reason {
        SkipReason::LauncherArgs => "invalid launcher arguments".to_owned(),
        SkipReason::NeedsRunner { proton: true } => "Proton runner, needs --runner".to_owned(),
        SkipReason::NeedsRunner { proton: false } => "Wine runner, needs --runner".to_owned(),
        SkipReason::NameTaken(name) => format!("name `{name}` is already taken"),
        SkipReason::Blocked(message)
        | SkipReason::Unreadable(message)
        | SkipReason::Unavailable(message) => message.clone(),
    }
}

/// `Some` skips asking: `--desktop` always answers yes, and `--yes` alone
/// (without `--desktop`) answers no. `None` means `--all` must ask.
pub(super) fn bulk_desktop_choice(desktop_flag: bool, yes_flag: bool) -> Option<bool> {
    if desktop_flag {
        Some(true)
    } else if yes_flag {
        Some(false)
    } else {
        None
    }
}

/// The warning every ready (non-skipped) row shares, if there is at least
/// one ready row and they all carry the exact same one. `None` also covers
/// there being no shared warning to pull out.
pub(super) fn shared_ready_warning(rows: &[BulkRow]) -> Option<&str> {
    let mut ready_warnings = rows.iter().filter_map(|row| match &row.status {
        RowStatus::Ready { warn } => Some(warn.as_deref()),
        RowStatus::ReadyNative { .. } | RowStatus::Skipped(_) => None,
    });
    let first = ready_warnings.next()??;
    ready_warnings
        .all(|warn| warn == Some(first))
        .then_some(first)
}

pub(super) fn render_bulk_table(
    runner_name: &str,
    mode_label: &str,
    rows: &[BulkRow],
    already: &[String],
) -> String {
    let shared_warning = shared_ready_warning(rows);
    let name_width = text::widest(rows.iter().map(|row| row.name.as_str()));
    let title_width = text::widest(rows.iter().map(|row| row.title.as_str()));
    let mode_width = rows
        .iter()
        .filter_map(|row| match row.status {
            RowStatus::ReadyNative { .. } => Some("native".len()),
            RowStatus::Ready { .. } => Some(mode_label.len()),
            RowStatus::Skipped(_) => None,
        })
        .max()
        .unwrap_or(0);
    let noun = text::plural(rows.len(), "game", "games");
    let heading = if runner_name.is_empty() {
        format!("Import {} Heroic {noun}:", rows.len())
    } else {
        format!("Import {} Heroic {noun} with {runner_name}:", rows.len())
    };
    let mut out = format!("{}\n", style::title(heading));
    for row in rows {
        let (mode, warn) = match &row.status {
            RowStatus::Skipped(reason) => (format!("skip: {reason}"), None),
            RowStatus::ReadyNative { warn } => ("native".to_owned(), warn.as_deref()),
            RowStatus::Ready { warn } => (
                mode_label.to_owned(),
                warn.as_deref().filter(|_| shared_warning.is_none()),
            ),
        };
        let mut parts = Vec::new();
        if row.launch_scripts && !matches!(row.status, RowStatus::Skipped(_)) {
            parts.push("runs launch scripts".to_owned());
        }
        if let Some(warning) = warn {
            parts.push(style::warning(format!("warn: {warning}")).to_string());
        }
        // Copy and native rows share the table, so the mode is padded for the
        // notes after it to line up.
        let status = if parts.is_empty() {
            mode
        } else {
            format!("{mode:mode_width$}   {}", parts.join("   "))
        };
        out.push_str(&format!(
            "  {}  {}  {status}\n",
            text::pad(&row.name, name_width),
            text::pad(&row.title, title_width)
        ));
    }
    if !already.is_empty() {
        out.push_str(&format!("Already imported: {}\n", already.join(", ")));
    }
    if let Some(warning) = shared_warning {
        out.push_str(&format!("{}\n", style::warning(format!("warn: {warning}"))));
    }
    out.pop();
    out
}

/// What every game in `--all` is imported with.
pub(super) struct Bulk<'a> {
    pub(super) paths: &'a Paths,
    pub(super) global: &'a GlobalConfig,
    pub(super) access: &'a Access<'a>,
    pub(super) source: &'a HeroicSource,
    pub(super) desktop: bool,
    /// The key to fetch art with, or None for no art.
    pub(super) art: Option<Key>,
    pub(super) verbose: bool,
}

fn import_all(bulk: &Bulk, games: Vec<Game>) -> anyhow::Result<ExitCode> {
    let paths = bulk.paths;
    let verbose = bulk.verbose;
    let total = games.len();
    let mut done = 0usize;
    let mut failed = Vec::new();
    let cancellation = Cancellation::new()?;
    let mut canceled = false;
    for game in games {
        if cancellation.token.is_canceled() {
            canceled = true;
            break;
        }
        let name = game.name().to_owned();
        // Art lines wait for the progress line to end.
        let (mut art_out, mut art_err) = (Vec::new(), Vec::new());
        let imported = match &game {
            Game::Windows(plan) => {
                if verbose {
                    let mut output = Prompt::new(NoInput, crate::output::stdout());
                    show_game_summary(&mut output, paths, plan.resolved(), verbose)?;
                }
                import_progress(&mut crate::output::stdout().lock(), &name, || {
                    import_one(bulk, plan, &cancellation, &mut art_out, &mut art_err)
                })?
            }
            Game::Native(plan) => {
                import_progress(&mut crate::output::stdout().lock(), &name, || {
                    native::import_one(bulk, plan, &mut art_out, &mut art_err).map(|()| None)
                })?
            }
        };
        crate::output::stdout().write_all(&art_out)?;
        crate::output::stderr().write_all(&art_err)?;
        match imported {
            Ok(log_body) => {
                if let Some(body) = log_body {
                    print_log_path(paths, &name, &body);
                }
                done += 1;
            }
            Err(error) if cancel::prompted(&error) => {
                canceled = true;
                break;
            }
            Err(_) => failed.push(name),
        }
    }
    let summary = text::games_summary("Imported", done, total);
    if failed.is_empty() {
        println!("{summary}");
    } else {
        println!("{summary} {} failed: {}", failed.len(), failed.join(", "));
    }
    if canceled {
        let remaining = total - done - failed.len();
        println!("{} canceled.", text::counted(remaining, "game", "games"));
        return Ok(cancel::exit_code());
    }
    Ok(if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

pub(super) fn import_progress<W: Write, T>(
    output: &mut W,
    name: &str,
    operation: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<anyhow::Result<T>> {
    write!(output, "Importing {name}...")?;
    output.flush()?;
    let result = operation();
    match &result {
        Ok(_) => writeln!(output, " done")?,
        Err(error) if cancel::prompted(error) => writeln!(output, " canceled")?,
        Err(error) => writeln!(output, " failed: {error:#}")?,
    }
    output.flush()?;
    Ok(result)
}

/// Imports one game of `--all`. Art is fetched with the top match, and its
/// lines and warnings go to `out` and `err`. Art failures never fail the game.
pub(super) fn import_one(
    bulk: &Bulk,
    game: &GamePlan,
    cancellation: &Cancellation,
    out: &mut Vec<u8>,
    err: &mut Vec<u8>,
) -> anyhow::Result<Option<String>> {
    let (paths, global) = (bulk.paths, bulk.global);
    let mut plan = game.clone();
    plan.set_desktop_enabled(bulk.desktop);
    let art = ArtChoice {
        key: bulk.art.clone(),
        enabled: bulk.art.is_some(),
        term: plan.heroic().title.clone(),
    };
    let log_body = bulk
        .verbose
        .then(|| plan.log_text(global, bulk.source) + &art.log_line());
    let registered = apply_plan_with_cancellation(paths, global, bulk.source, &plan, cancellation)?;
    report_playtime(&registered.playtime);
    let environment = &registered.environment;
    let top = Pick::<NoInput, Vec<u8>>::Top(out);
    // A top match can't be canceled.
    let _ = fetch_art(
        paths,
        global,
        bulk.access,
        &environment.name,
        &art,
        top,
        err,
    );
    finish_registered(paths, global, &registered)?;
    Ok(log_body)
}
