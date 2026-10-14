//! The guided import's final review and its per-field editors.

use anyhow::bail;
use std::io::Write;
use wineo_core::config::GlobalConfig;
use wineo_core::heroic::{HeroicIssueKind, HeroicSource};
use wineo_core::heroic_import::{DesktopEntry, GamePlan, InputKind, Policy};
use wineo_core::paths::Paths;

use super::summary::{display_args, env_count_label, final_facts_text, on_off};
use super::{ArtChoice, print_issue};
use crate::commands::table;
use crate::picker;
use crate::prompt::{Canceled, Input, Prompt, Reply, numbered};
use crate::style;

#[derive(Clone, Copy)]
enum EditField {
    Executable,
    WorkingDirectory,
    Arguments,
    Environment,
    GameMode,
    Name,
    Destination,
    Desktop,
    Art,
}

/// Bundles what every review function needs but doesn't itself change, to
/// stay under clippy's argument-count limit.
pub(super) struct ReviewContext<'a> {
    pub(super) paths: &'a Paths,
    pub(super) global: &'a GlobalConfig,
    pub(super) source: &'a HeroicSource,
    pub(super) verbose: bool,
}

pub(super) fn final_review<I: Input, W: Write>(
    ctx: &ReviewContext,
    plan: &mut GamePlan,
    art: &mut ArtChoice,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let paths = ctx.paths;
    let global = ctx.global;
    let verbose = ctx.verbose;
    loop {
        let facts = final_facts_text(ctx, plan, verbose);
        let heading = format!("{facts}\n");

        let imported = plan.resolved();
        let cwd_display = match &imported.cwd {
            Some(cwd) => style::path(paths, cwd),
            None => "default".to_owned(),
        };
        let copy_to_display = if plan.is_adopt() {
            "adopt in place".to_owned()
        } else {
            style::path(paths, plan.destination())
        };
        let desktop_display = if plan.desktop().enabled { "yes" } else { "no" }.to_owned();
        let mut items: Vec<(&str, String, EditField)> = vec![
            (
                "exe",
                style::path(paths, &imported.exe),
                EditField::Executable,
            ),
            ("working dir", cwd_display, EditField::WorkingDirectory),
            (
                "arguments",
                display_args(&imported.args),
                EditField::Arguments,
            ),
            (
                "env",
                env_count_label(&imported.env),
                EditField::Environment,
            ),
            (
                "gamemode",
                on_off(imported.gamemode.unwrap_or(false)).to_owned(),
                EditField::GameMode,
            ),
            ("name", plan.name().to_owned(), EditField::Name),
            ("copy to", copy_to_display, EditField::Destination),
            ("desktop", desktop_display, EditField::Desktop),
        ];
        let mut note = "Close Heroic and the game first.".to_owned();
        if art.key.is_some() {
            items.push(("art", art.display(), EditField::Art));
        }
        let width = table::label_column(items.iter().map(|(label, _, _)| *label));
        if art.key.is_none() {
            // Unnumbered without a key, lined up with the numbered rows.
            let indent = " ".repeat(items.len().to_string().len() + 4);
            note = format!("{indent}{:width$}{}\n{note}", "art", art.display());
        }
        let choices = numbered(
            items
                .into_iter()
                .map(|(label, value, field)| (format!("{label:width$}{value}"), field)),
        );
        let choice = prompt.review(&heading, Some(&note), "import", &choices, true)?;
        match choice {
            Reply::Value(None) => return Ok(()),
            Reply::Value(Some(field)) => match field {
                EditField::Executable => edit_executable(paths, plan, prompt)?,
                EditField::WorkingDirectory => edit_working_directory(paths, plan, prompt)?,
                EditField::Arguments => edit_arguments(plan, prompt)?,
                EditField::Environment => edit_environment(plan, prompt)?,
                EditField::GameMode => edit_gamemode(plan, prompt)?,
                EditField::Name => edit_name(paths, global, plan, prompt)?,
                EditField::Destination => edit_destination(paths, global, plan, prompt)?,
                EditField::Desktop => {
                    let title = plan.resolved().title.clone();
                    let edited = edit_desktop(paths, &title, plan.desktop().clone(), prompt)?;
                    plan.set_desktop_enabled(edited.enabled);
                    if edited.enabled {
                        plan.set_desktop_name(&edited.name)?;
                    }
                }
                EditField::Art => edit_art(art, prompt)?,
            },
            Reply::Back => return Err(Canceled.into()),
        }
    }
}

pub(super) fn edit_executable<I: Input, W: Write>(
    paths: &Paths,
    plan: &mut GamePlan,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let current = plan.resolved().exe.display().to_string();
    match prompt.text("Executable path", Some(&current), |value| {
        if value.trim().is_empty() {
            Err("Executable path cannot be empty".to_owned())
        } else {
            Ok(value.to_owned())
        }
    })? {
        Reply::Value(value) => {
            let exe = plan.resolve_path(paths, &picker::clean_path(&value))?;
            plan.set_executable(exe)?;
        }
        Reply::Back => {}
    }
    Ok(())
}

pub(super) fn edit_working_directory<I: Input, W: Write>(
    paths: &Paths,
    plan: &mut GamePlan,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let current = plan
        .resolved()
        .cwd
        .as_deref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    match prompt.text(
        "Working directory (empty for default)",
        Some(&current),
        |value| Ok(value.to_owned()),
    )? {
        Reply::Value(value) if value.is_empty() => plan.set_working_directory(None),
        Reply::Value(value) => {
            let cwd = plan.resolve_path(paths, &picker::clean_path(&value))?;
            plan.set_working_directory(Some(cwd));
        }
        Reply::Back => {}
    }
    Ok(())
}

fn edit_arguments<I: Input, W: Write>(
    plan: &mut GamePlan,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let current = plan.resolved().launcher_args.clone();
    match prompt.text("Launch arguments", Some(&current), |value| {
        wineo_core::heroic::parse_launcher_args(value).map(|_| value.to_owned())
    })? {
        Reply::Value(value) => plan.set_launcher_args(&value)?,
        Reply::Back => {}
    }
    Ok(())
}

fn edit_environment<I: Input, W: Write>(
    plan: &mut GamePlan,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    prompt.say("Enter environment values as KEY=VALUE, one per line. An empty line keeps the current values.")?;
    let Reply::Value(values) = prompt.list("environment")? else {
        return Ok(());
    };
    if values.is_empty() {
        return Ok(());
    }
    plan.set_environment(values)?;
    Ok(())
}

fn edit_gamemode<I: Input, W: Write>(
    plan: &mut GamePlan,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let current = plan.resolved().gamemode.unwrap_or(false);
    match prompt.confirm("Enable GameMode?", current)? {
        Reply::Value(value) => plan.set_gamemode(value),
        Reply::Back => {}
    }
    Ok(())
}

fn edit_name<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    plan: &mut GamePlan,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let current = plan.name().to_owned();
    match prompt.text("Environment name", Some(&current), |value| {
        picker::name_problem(value).map_or_else(|| Ok(value.to_owned()), Err)
    })? {
        Reply::Value(value) => plan.set_name(paths, global, &value)?,
        Reply::Back => {}
    }
    Ok(())
}

fn edit_destination<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    plan: &mut GamePlan,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    let adopt = match prompt.choose(
        "Prefix import mode",
        "Mode",
        &numbered([
            ("Copy into wineo".to_owned(), false),
            ("Adopt the original files in place".to_owned(), true),
        ]),
        Some(if plan.is_adopt() { "2" } else { "1" }),
        true,
    )? {
        Reply::Value(value) => value,
        Reply::Back => return Ok(()),
    };
    if adopt {
        plan.set_adopt();
        return Ok(());
    }
    plan.set_copy(paths, global, None)?;
    let default = plan.destination().display().to_string();
    match prompt.text("Copy destination", Some(&default), |value| {
        paths
            .absolute(&picker::clean_path(value))
            .map_err(|error| error.to_string())
    })? {
        Reply::Value(value) => plan.set_copy(paths, global, Some(value))?,
        Reply::Back => {}
    }
    Ok(())
}

pub(super) fn edit_desktop<I: Input, W: Write>(
    paths: &Paths,
    title: &str,
    mut desktop: DesktopEntry,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<DesktopEntry> {
    if let Err(err) = wineo_core::desktop::supported(paths) {
        desktop.enabled = false;
        prompt.say(format!("{err}."))?;
        return Ok(desktop);
    }
    match prompt.confirm("Create a desktop entry?", desktop.enabled)? {
        Reply::Value(true) => {
            let current = if desktop.name.is_empty() {
                title.to_owned()
            } else {
                desktop.name.clone()
            };
            match prompt.text("Desktop entry name", Some(&current), |value| {
                if value.is_empty() {
                    Err("Enter a name".to_owned())
                } else {
                    Ok(value.to_owned())
                }
            })? {
                Reply::Value(name) => {
                    desktop.enabled = true;
                    desktop.name = name;
                }
                Reply::Back => {}
            }
        }
        Reply::Value(false) => desktop.enabled = false,
        Reply::Back => {}
    }
    Ok(desktop)
}

pub(super) fn edit_art<I: Input, W: Write>(
    art: &mut ArtChoice,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<()> {
    match prompt.confirm("Fetch art from SteamGridDB?", art.enabled)? {
        Reply::Value(true) => {
            match prompt.text("Search for", Some(&art.term), |value| {
                if value.is_empty() {
                    Err("Enter a search term.".to_owned())
                } else {
                    Ok(value.to_owned())
                }
            })? {
                Reply::Value(term) => {
                    art.enabled = true;
                    art.term = term;
                }
                Reply::Back => {}
            }
        }
        Reply::Value(false) => art.enabled = false,
        Reply::Back => {}
    }
    Ok(())
}

pub(super) fn reject_unresolved(plan: &GamePlan) -> anyhow::Result<()> {
    let blocking: Vec<_> = plan.blocking().collect();
    if !blocking.is_empty() {
        for finding in blocking {
            print_issue("unresolved", &finding.issue);
        }
        bail!("Heroic launch settings need review; rerun with --interactive");
    }
    Ok(())
}

pub(super) fn review_settings<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    plan: &mut GamePlan,
) -> anyhow::Result<()> {
    // The summary already showed the dropped conflicts, the omitted settings,
    // and the ones needing attention as warnings, and the final review edits
    // the paths that need it. Only what can't be imported blocks, or needs an
    // answer.
    plan.resolve(Policy::Defaults);
    if plan
        .blocking()
        .any(|finding| finding.issue.kind == HeroicIssueKind::Invalid)
    {
        bail!("Heroic has invalid launch settings that cannot be imported automatically");
    }
    if plan.needs_input() == Some(InputKind::LauncherArgs) {
        let current = plan.resolved().launcher_args.clone();
        let args = match prompt.text("Correct launcher arguments", Some(&current), |value| {
            wineo_core::heroic::parse_launcher_args(value).map(|_| value.to_owned())
        })? {
            Reply::Value(value) => value,
            Reply::Back => return Err(anyhow::anyhow!(Canceled)),
        };
        plan.set_launcher_args(&args)?;
    }
    if plan.blocking().next().is_some() {
        bail!("Heroic import still has unresolved settings");
    }
    Ok(())
}
