//! Rendering what a Heroic game imports with, for the summary, review, and bulk table.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use wineo_core::config::GlobalConfig;
use wineo_core::gamescope::GamescopeSetting;
use wineo_core::heroic::{
    HeroicImport, HeroicIssue, HeroicIssueKind, HeroicRunnerKind, HeroicSource, SettingsReview,
};
use wineo_core::heroic_import::{
    COVERED_BY_PROTON, GamePlan, Warning, is_dropped_conflict, redact_env_value,
    runner_dependency_lines, shared_prefix_lines,
};
use wineo_core::import::ExternalDependency;
use wineo_core::paths::Paths;

use super::ReviewContext;
use crate::commands::external_runners;
use crate::commands::table;
use crate::prompt::{Input, Prompt};
use crate::style;
use crate::text;

/// One labeled row of a rendered block, and any continuation lines under it.
pub(super) struct Block {
    pub(super) label: &'static str,
    pub(super) lines: Vec<String>,
}

pub(super) fn render_blocks(blocks: &[Block]) -> String {
    let width = table::label_column(blocks.iter().map(|block| block.label));
    let mut out = String::new();
    for block in blocks {
        for (index, line) in block.lines.iter().enumerate() {
            let label = if index == 0 { block.label } else { "" };
            out.push_str(&format!("  {label:width$}{line}\n"));
        }
    }
    out.pop();
    out
}

pub(super) fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

pub(super) fn display_args(args: &[String]) -> String {
    if args.is_empty() {
        return "none".to_owned();
    }
    args.iter()
        .map(|arg| {
            if arg.is_empty() || arg.contains(char::is_whitespace) || arg.contains(['"', '\\']) {
                format!("{arg:?}")
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn env_count_label(env: &BTreeMap<String, String>) -> String {
    match env.len() {
        0 => "none".to_owned(),
        1 => "1 variable".to_owned(),
        n => format!("{n} variables"),
    }
}

/// Merges `enableEsync`/`enableFsync` into one row when they agree, which is
/// the common case, and keeps them apart when they don't.
fn sync_block(review: &SettingsReview) -> Option<Block> {
    let find = |field: &str| {
        review
            .carried
            .iter()
            .find(|item| item.field == field)
            .map(|item| item.detail == "true")
    };
    match (find("enableEsync"), find("enableFsync")) {
        (Some(esync), Some(fsync)) if esync == fsync => Some(Block {
            label: "esync/fsync",
            lines: vec![on_off(esync).to_owned()],
        }),
        (Some(esync), Some(fsync)) => Some(Block {
            label: "esync/fsync",
            lines: vec![format!("esync {}, fsync {}", on_off(esync), on_off(fsync))],
        }),
        (Some(esync), None) => Some(Block {
            label: "esync",
            lines: vec![on_off(esync).to_owned()],
        }),
        (None, Some(fsync)) => Some(Block {
            label: "fsync",
            lines: vec![on_off(fsync).to_owned()],
        }),
        (None, None) => None,
    }
}

pub(super) fn hook_blocks(review: &SettingsReview) -> Vec<Block> {
    review
        .hooks()
        .into_iter()
        .map(|(label, path)| Block {
            label,
            lines: vec![path],
        })
        .collect()
}

pub(super) fn gamescope_display(setting: &GamescopeSetting) -> String {
    match setting {
        GamescopeSetting::Off => "off".to_owned(),
        GamescopeSetting::On(game) => {
            let args = game.args(false);
            if args.is_empty() {
                "on".to_owned()
            } else {
                format!("on: {}", display_args(&args))
            }
        }
    }
}

/// Proton handles a prefix with both registry layouts, so the conflict
/// Heroic's reader raises for it is not worth showing before the runner is
/// picked. It is dropped for real once one is.
fn covered_prefix_conflict(imported: &HeroicImport, issue: &HeroicIssue) -> bool {
    imported.runner.kind == HeroicRunnerKind::Proton
        && issue.kind == HeroicIssueKind::Conflict
        && issue.field.as_deref() == Some("winePrefix")
}

/// The text a warning has in the bulk table and the final review. Core only
/// says what the warning is, this is the wording.
fn warning_text(warning: &Warning, adopt: bool) -> String {
    match warning {
        Warning::SharedPrefix(shared) => shared_prefix_lines(shared, adopt, false).join(" "),
        Warning::UmuDisabled(message) | Warning::NvapiNotInstalled(message) => message.clone(),
        Warning::AntiCheatNotImported => "EAC/BattlEye runtimes aren't imported".to_owned(),
        Warning::Omitted(count) => {
            format!(
                "{count} unsupported setting{} omitted",
                text::plural(*count, "", "s")
            )
        }
        Warning::Dropped(count) => {
            format!(
                "{count} managed conflict{} dropped",
                text::plural(*count, "", "s")
            )
        }
        Warning::NeedsAttention(_) => "settings need attention, see -v".to_owned(),
    }
}

/// Every warning on one line, or None when there is nothing to warn about.
pub(super) fn warnings_line(warnings: &[Warning], adopt: bool) -> Option<String> {
    let texts: Vec<String> = warnings
        .iter()
        .map(|warning| warning_text(warning, adopt))
        .collect();
    (!texts.is_empty()).then(|| texts.join("; "))
}

pub(super) fn show_game_summary<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    paths: &Paths,
    imported: &HeroicImport,
    verbose: bool,
) -> anyhow::Result<()> {
    let title = style::title(format!("{} (Heroic {})", imported.title, imported.id));
    let mut blocks = Vec::new();

    let mut runner_label = imported.runner.name.clone();
    if imported.runner.source.is_some() {
        runner_label.push_str(" (Heroic's copy)");
    }
    blocks.push(Block {
        label: "runner",
        lines: vec![runner_label],
    });
    blocks.push(Block {
        label: "exe",
        lines: vec![style::path(paths, &imported.exe)],
    });
    if !imported.args.is_empty() {
        blocks.push(Block {
            label: "arguments",
            lines: vec![display_args(&imported.args)],
        });
    }
    if let Some(cwd) = &imported.cwd {
        blocks.push(Block {
            label: "working dir",
            lines: vec![style::path(paths, cwd)],
        });
    }
    if let Some(gamemode) = imported.gamemode {
        blocks.push(Block {
            label: "gamemode",
            lines: vec![on_off(gamemode).to_owned()],
        });
    }
    if imported.enable_wine_wayland || imported.wayland == Some(false) {
        blocks.push(Block {
            label: "wayland",
            lines: vec![
                if imported.wayland == Some(false) {
                    "off"
                } else if imported.wayland == Some(true) {
                    "on"
                } else {
                    "requested"
                }
                .to_owned(),
            ],
        });
    }
    if let Some(mangohud) = imported.mangohud {
        blocks.push(Block {
            label: "mangohud",
            lines: vec![on_off(mangohud).to_owned()],
        });
    }
    if let Some(gamescope) = &imported.gamescope {
        blocks.push(Block {
            label: "gamescope",
            lines: vec![gamescope_display(gamescope)],
        });
    }
    blocks.extend(hook_blocks(&imported.settings_review));
    if let Some(dxvk) = &imported.dxvk {
        blocks.push(Block {
            label: "dxvk",
            lines: vec![dxvk.clone()],
        });
    }
    if let Some(vkd3d) = &imported.vkd3d {
        blocks.push(Block {
            label: "vkd3d",
            lines: vec![vkd3d.clone()],
        });
    }
    if let Some(sync) = sync_block(&imported.settings_review) {
        blocks.push(sync);
    }
    if !imported.env.is_empty() {
        blocks.push(Block {
            label: "env",
            lines: imported
                .env
                .iter()
                .map(|(key, value)| format!("{key}={}", redact_env_value(key, value)))
                .collect(),
        });
    }

    let mut warnings = Vec::new();
    if imported.issues.iter().any(|issue| {
        matches!(
            issue.field.as_deref(),
            Some("eacRuntime") | Some("battlEyeRuntime")
        )
    }) {
        warnings.push("EAC and BattlEye runtimes aren't imported".to_owned());
    }
    for issue in &imported.issues {
        if matches!(
            issue.field.as_deref(),
            Some("eacRuntime") | Some("battlEyeRuntime")
        ) {
            continue;
        }
        let field = issue.field.as_deref().unwrap_or("?");
        match issue.kind {
            HeroicIssueKind::NeedsAttention if verbose => {
                warnings.push(format!("{field}: {}", issue.message));
            }
            HeroicIssueKind::NeedsAttention => warnings.push(issue.message.clone()),
            HeroicIssueKind::Unsupported if verbose => {
                warnings.push(format!("{field}: {}", issue.message));
            }
            HeroicIssueKind::Unsupported if field == "disableUMU" => {
                warnings.push(issue.message.clone());
            }
            HeroicIssueKind::Conflict if is_dropped_conflict(issue) => {
                warnings.push(format!("{field} is dropped, wineo manages it"));
            }
            _ => {}
        }
    }
    let hidden = imported
        .issues
        .iter()
        .filter(|issue| {
            issue.kind == HeroicIssueKind::Unsupported
                && issue.field.as_deref() != Some("disableUMU")
                && !matches!(
                    issue.field.as_deref(),
                    Some("eacRuntime") | Some("battlEyeRuntime")
                )
                && !issue
                    .field
                    .as_deref()
                    .is_some_and(|field| COVERED_BY_PROTON.contains(&field))
        })
        .count();
    if !verbose && hidden > 0 {
        warnings.push(format!(
            "{hidden} more Heroic {} not imported, -v lists them",
            text::plural(hidden, "setting is", "settings are")
        ));
    }
    if !warnings.is_empty() {
        blocks.push(Block {
            label: "warning",
            lines: warnings.iter().map(style::warning).collect(),
        });
    }

    let errors: Vec<String> = imported
        .issues
        .iter()
        .filter(|issue| {
            matches!(
                issue.kind,
                HeroicIssueKind::Missing | HeroicIssueKind::Invalid | HeroicIssueKind::Conflict
            ) && !covered_prefix_conflict(imported, issue)
                && !is_dropped_conflict(issue)
        })
        .map(|issue| style::error(&issue.message))
        .collect();
    if !errors.is_empty() {
        blocks.push(Block {
            label: "error",
            lines: errors,
        });
    }

    Ok(prompt.say(format!("{title}\n{}", render_blocks(&blocks)))?)
}

/// The facts the final review shows above the editable settings: what stays
/// where it is, and what the runner and links resolve to.
pub(super) fn final_facts_text(ctx: &ReviewContext, plan: &GamePlan, verbose: bool) -> String {
    let paths = ctx.paths;
    let global = ctx.global;
    let source = ctx.source;
    let imported = plan.resolved();
    let inspection = plan.inspection();
    let request = plan.request();
    let resolved = plan.runner();
    let title = style::title(format!("{} -> {}", imported.title, request.name));
    let mut blocks = Vec::new();

    let prefix_note = if request.adopt {
        "(adopted, launches modify it)"
    } else {
        "(left in place)"
    };
    blocks.push(Block {
        label: "prefix",
        lines: vec![format!(
            "{} {prefix_note}",
            style::path(paths, &inspection.source_root)
        )],
    });

    let runner_label = external_runners::with_owner(
        paths,
        resolved,
        if request.runner == resolved.name() {
            resolved.name().to_owned()
        } else {
            format!("{} -> {}", request.runner, resolved.name())
        },
    );
    blocks.push(Block {
        label: "runner",
        lines: vec![runner_label],
    });
    if imported.enable_wine_wayland || imported.wayland == Some(false) {
        blocks.push(Block {
            label: "wayland",
            lines: vec![
                if imported.wayland == Some(false) {
                    "off"
                } else if imported.wayland == Some(true) {
                    "on"
                } else {
                    "not imported"
                }
                .to_owned(),
            ],
        });
    }
    if let Some(mangohud) = imported.mangohud {
        blocks.push(Block {
            label: "mangohud",
            lines: vec![on_off(mangohud).to_owned()],
        });
    }
    if let Some(gamescope) = &imported.gamescope {
        blocks.push(Block {
            label: "gamescope",
            lines: vec![gamescope_display(gamescope)],
        });
    }
    blocks.extend(hook_blocks(&imported.settings_review));
    if let Some(dxvk) = &imported.dxvk {
        blocks.push(Block {
            label: "dxvk",
            lines: vec![dxvk.clone()],
        });
    }
    if let Some(vkd3d) = &imported.vkd3d {
        blocks.push(Block {
            label: "vkd3d",
            lines: vec![vkd3d.clone()],
        });
    }
    if !imported.args.is_empty() {
        blocks.push(Block {
            label: "arguments",
            lines: vec![display_args(&imported.args)],
        });
    }

    let shared = shared_prefix_lines(plan.shared_prefix(), request.adopt, verbose);
    if !shared.is_empty() {
        blocks.push(Block {
            label: "shared prefix",
            lines: shared,
        });
    }
    let runner_deps = runner_dependency_lines(plan.runner_dependencies(), verbose);
    if !runner_deps.is_empty() {
        blocks.push(Block {
            label: "runner links",
            lines: runner_deps,
        });
    }
    let review_warnings: Vec<Warning> = plan
        .warnings()
        .into_iter()
        .filter(|warning| !matches!(warning, Warning::SharedPrefix(_)))
        .collect();
    if let Some(warn) = warnings_line(&review_warnings, request.adopt) {
        blocks.push(Block {
            label: "warning",
            lines: vec![style::warning(warn)],
        });
    }
    if verbose {
        let detail: Vec<String> = imported
            .issues
            .iter()
            .filter(|issue| {
                matches!(
                    issue.kind,
                    HeroicIssueKind::Conflict
                        | HeroicIssueKind::Unsupported
                        | HeroicIssueKind::NeedsAttention
                )
            })
            .map(|issue| {
                format!(
                    "{}: {}",
                    issue.field.as_deref().unwrap_or("setting"),
                    issue.message
                )
            })
            .collect();
        if !detail.is_empty() {
            blocks.push(Block {
                label: "review",
                lines: detail,
            });
        }
    }

    if imported.external_dependencies.contains(&imported.exe)
        && let Some(dir) = imported.exe.parent()
    {
        blocks.push(Block {
            label: "game dir",
            lines: vec![format!("{} (stays in place)", style::path(paths, dir))],
        });
    }

    let links = link_lines(paths, source, &inspection.external_dependencies, verbose);
    if !links.is_empty() {
        blocks.push(Block {
            label: "links",
            lines: links,
        });
    }

    if global.gamemode.is_some() || !global.env.is_empty() {
        blocks.push(global_block(global, verbose));
    }

    format!("{title}\n{}", render_blocks(&blocks))
}

fn global_block(global: &GlobalConfig, verbose: bool) -> Block {
    if verbose {
        let mut lines = Vec::new();
        if let Some(enabled) = global.gamemode {
            lines.push(format!("gamemode {}", on_off(enabled)));
        }
        for (key, value) in &global.env {
            lines.push(format!("{key}={}", redact_env_value(key, value)));
        }
        Block {
            label: "global",
            lines,
        }
    } else {
        let mut parts = Vec::new();
        if global.gamemode.is_some() {
            parts.push("gamemode".to_owned());
        }
        if !global.env.is_empty() {
            parts.push(env_count_label(&global.env));
        }
        Block {
            label: "global",
            lines: vec![format!("{} apply unless overridden", parts.join(", "))],
        }
    }
}

/// The standard `dosdevices` drive links aren't shown; they're wineo's own
/// doing, not something Heroic left behind.
fn is_dosdevices(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "dosdevices")
}

pub(super) fn link_lines(
    paths: &Paths,
    source: &HeroicSource,
    deps: &[ExternalDependency],
    verbose: bool,
) -> Vec<String> {
    let visible: Vec<&ExternalDependency> = deps
        .iter()
        .filter(|dep| !is_dosdevices(&dep.source))
        .collect();
    if visible.is_empty() {
        return Vec::new();
    }
    if verbose {
        return visible
            .iter()
            .map(|dep| {
                format!(
                    "{} -> {}",
                    style::path(paths, &dep.source),
                    style::path(paths, &dep.target)
                )
            })
            .collect();
    }
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    for dep in &visible {
        *groups
            .entry(link_group_label(paths, source.config_dir(), &dep.target))
            .or_insert(0) += 1;
    }
    groups
        .into_iter()
        .map(|(label, count)| {
            format!(
                "{count} prefix file{} point into {label}",
                text::plural(count, "", "s")
            )
        })
        .collect()
}

/// Groups links that land in Heroic's own copy of a runner under its name,
/// so a font or ICU symlink flood collapses to one line.
fn link_group_label(paths: &Paths, config_dir: &Path, target: &Path) -> String {
    let tools = config_dir.join("tools");
    if let Ok(rest) = target.strip_prefix(&tools)
        && let Some(name) = rest
            .components()
            .nth(1)
            .and_then(|c| c.as_os_str().to_str())
    {
        return format!("Heroic's {name}");
    }
    style::path(paths, target.parent().unwrap_or(target))
}
