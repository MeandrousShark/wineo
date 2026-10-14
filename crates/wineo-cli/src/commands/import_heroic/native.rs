//! Importing a native sideloaded Heroic game as a native environment. There
//! is no prefix or runner, so this skips everything about them and ignores
//! the Wine settings Heroic keeps for native games too.

use std::io::Write;
use std::process::ExitCode;

use wineo_core::config::GlobalConfig;
use wineo_core::heroic::{HeroicCandidate, HeroicIssueKind, HeroicNativeImport, HeroicSource};
use wineo_core::heroic_import::{NativePlan, apply_native, plan_native, redact_env_value};
use wineo_core::paths::Paths;

use super::bulk::Bulk;
use super::review::{edit_art, edit_desktop};
use super::summary::{Block, display_args, gamescope_display, hook_blocks, on_off, render_blocks};
use super::{Args, ArtChoice, fetch_art, print_issue, review_art};
use crate::cancel;
use crate::commands::art::{Access, Pick};
use crate::commands::import::finish_guided;
use crate::commands::table;
use crate::picker;
use crate::prompt::{self, Canceled, Input, NoInput, Prompt, Reply, Terminal, numbered};
use crate::style;

/// Refuses the flags that only mean something for a Windows game.
fn refuse_wine_flags(args: &Args, title: &str) -> anyhow::Result<()> {
    let flag = if args.runner.is_some() {
        "--runner"
    } else if args.adopt {
        "--adopt"
    } else if args.dir.is_some() {
        "--dir"
    } else if args.arch.is_some() {
        "--arch"
    } else {
        return Ok(());
    };
    anyhow::bail!("{title} is a native game, so {flag} can't be used")
}

/// The game as it will be imported, with what Heroic had that isn't.
fn summary(paths: &Paths, imported: &HeroicNativeImport, name: &str) -> String {
    let title = style::title(format!("Import {} as {name}", imported.title));
    let mut blocks = vec![
        Block {
            label: "kind",
            lines: vec!["native".to_owned()],
        },
        Block {
            label: "exe",
            lines: vec![style::path(paths, &imported.exe)],
        },
    ];
    if !imported.args.is_empty() {
        blocks.push(Block {
            label: "arguments",
            lines: vec![display_args(&imported.args)],
        });
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
    blocks.push(Block {
        label: "gamemode",
        lines: vec![on_off(imported.gamemode.unwrap_or(false)).to_owned()],
    });
    if imported.mangohud == Some(true) {
        blocks.push(Block {
            label: "mangohud",
            lines: vec!["on".to_owned()],
        });
    }
    if let Some(gamescope) = &imported.gamescope {
        blocks.push(Block {
            label: "gamescope",
            lines: vec![gamescope_display(gamescope)],
        });
    }
    blocks.extend(hook_blocks(&imported.settings_review));
    let warnings: Vec<String> = imported
        .issues
        .iter()
        .filter(|issue| {
            matches!(
                issue.kind,
                HeroicIssueKind::Unsupported
                    | HeroicIssueKind::NeedsAttention
                    | HeroicIssueKind::Conflict
            )
        })
        .map(|issue| {
            style::warning(match &issue.field {
                Some(field) => format!("{field}: {}", issue.message),
                None => issue.message.clone(),
            })
        })
        .collect();
    if !warnings.is_empty() {
        blocks.push(Block {
            label: "warning",
            lines: warnings,
        });
    }
    format!("{title}\n{}", render_blocks(&blocks))
}

/// Imports a native game with everything given as flags.
pub(super) fn direct(
    paths: &Paths,
    global: &GlobalConfig,
    access: &Access,
    source: &HeroicSource,
    args: &Args,
) -> anyhow::Result<ExitCode> {
    let game = args.game.as_deref().expect("checked by the caller");
    let name = args.name.clone().expect("checked by the caller");
    let mut plan = plan_native(source, game, Some(name))?;
    refuse_wine_flags(args, &plan.heroic().title)?;
    plan.reject_invalid()?;
    plan.check_exe(paths, global)?;
    plan.set_desktop_enabled(args.desktop);
    let art = if !args.dry_run && args.art && !global.art.fetch.kinds().is_empty() {
        let check = (access.check_key)();
        check.print_skip();
        ArtChoice::checked(check, &plan.heroic().title)
    } else {
        ArtChoice::off()
    };
    println!("{}", summary(paths, plan.resolved(), plan.name()));
    if args.dry_run {
        super::super::import::check_name(paths, plan.name())?;
        if args.art {
            access.review(
                &plan.heroic().title,
                global.art.fetch,
                &mut crate::output::stdout(),
                &mut crate::output::stderr(),
            );
        }
        println!("Dry run, nothing was imported.");
        return Ok(ExitCode::SUCCESS);
    }
    let registered = apply_native(paths, source, &plan)?;
    super::report_playtime(&registered.playtime);
    let environment = &registered.environment;
    let mut prompt = (art.enabled && prompt::interactive())
        .then(|| Terminal::open().ok())
        .flatten()
        .map(|input| Prompt::new(input, crate::output::stdout()));
    let mut stdout = crate::output::stdout();
    let pick = match &mut prompt {
        Some(prompt) => Pick::Ask(prompt),
        None => Pick::Top(&mut stdout),
    };
    let fetched = fetch_art(
        paths,
        global,
        access,
        &environment.name,
        &art,
        pick,
        &mut crate::output::stderr(),
    );
    drop(prompt);
    super::finish_registered(paths, global, &registered)?;
    if fetched.is_err() {
        let name = &environment.name;
        eprintln!("Canceled. Kept {name}, continue with `wineo setup {name}`.");
        return Ok(cancel::exit_code());
    }
    println!(
        "Imported {}. Add or edit targets with wineo setup {}",
        environment.name, environment.name
    );
    Ok(ExitCode::SUCCESS)
}

#[derive(Clone, Copy)]
enum Field {
    Name,
    GameMode,
    Desktop,
    Art,
}

/// Guided import of a native game picked from Heroic's list.
pub(super) fn guided<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    access: &Access,
    source: &HeroicSource,
    candidate: &HeroicCandidate,
    args: &Args,
    prompt: &mut Prompt<I, W>,
) -> anyhow::Result<ExitCode> {
    let mut plan = plan_native(source, &candidate.id, None)?;
    refuse_wine_flags(args, &plan.heroic().title)?;
    plan.reject_invalid()?;
    let suggested = plan.name().to_owned();
    let name = match prompt.text(
        "Environment name",
        Some(args.name.as_deref().unwrap_or(&suggested)),
        |value| picker::name_problem(value).map_or_else(|| Ok(value.to_owned()), Err),
    )? {
        Reply::Value(value) => value,
        Reply::Back => return Err(Canceled.into()),
    };
    plan.set_name(&name)?;
    plan.set_desktop_enabled(paths.portable_root().is_none());
    let mut art = review_art(global, access, prompt, &plan.heroic().title)?;
    loop {
        let heading = format!("{}\n", summary(paths, plan.resolved(), plan.name()));
        let mut items = vec![
            ("name", plan.name().to_owned(), Field::Name),
            (
                "gamemode",
                on_off(plan.resolved().gamemode.unwrap_or(false)).to_owned(),
                Field::GameMode,
            ),
            (
                "desktop",
                if plan.desktop().enabled { "yes" } else { "no" }.to_owned(),
                Field::Desktop,
            ),
        ];
        if art.key.is_some() {
            items.push(("art", art.display(), Field::Art));
        }
        let width = table::label_column(items.iter().map(|(label, _, _)| *label));
        let choices = numbered(
            items
                .into_iter()
                .map(|(label, value, field)| (format!("{label:width$}{value}"), field)),
        );
        match prompt.review(&heading, None, "import", &choices, true)? {
            Reply::Value(None) => break,
            Reply::Value(Some(Field::Name)) => {
                let current = plan.name().to_owned();
                if let Reply::Value(value) =
                    prompt.text("Environment name", Some(&current), |value| {
                        picker::name_problem(value).map_or_else(|| Ok(value.to_owned()), Err)
                    })?
                {
                    plan.set_name(&value)?;
                }
            }
            Reply::Value(Some(Field::GameMode)) => {
                let current = plan.resolved().gamemode.unwrap_or(false);
                if let Reply::Value(value) = prompt.confirm("Enable GameMode?", current)? {
                    plan.set_gamemode(value);
                }
            }
            Reply::Value(Some(Field::Desktop)) => {
                let title = plan.heroic().title.clone();
                let edited = edit_desktop(paths, &title, plan.desktop().clone(), prompt)?;
                plan.set_desktop_enabled(edited.enabled);
                if edited.enabled {
                    plan.set_desktop_name(&edited.name)?;
                }
            }
            Reply::Value(Some(Field::Art)) => edit_art(&mut art, prompt)?,
            Reply::Back => return Err(Canceled.into()),
        }
    }
    plan.check_exe(paths, global)?;
    let registered = apply_native(paths, source, &plan)?;
    super::report_playtime(&registered.playtime);
    let environment = &registered.environment;
    let fetched = fetch_art(
        paths,
        global,
        access,
        &environment.name,
        &art,
        Pick::Ask(prompt),
        &mut crate::output::stderr(),
    );
    super::finish_registered(paths, global, &registered)?;
    if fetched.is_err() {
        prompt.say(format!(
            "Kept {}. Continue with `wineo setup {}`",
            environment.name, environment.name
        ))?;
        return Err(Canceled.into());
    }
    finish_guided(paths, global, environment, prompt)
}

/// Imports one native game of `--all`, like `import_one` does a Windows one.
pub(super) fn import_one(
    bulk: &Bulk,
    game: &NativePlan,
    out: &mut Vec<u8>,
    err: &mut Vec<u8>,
) -> anyhow::Result<()> {
    let (paths, global) = (bulk.paths, bulk.global);
    let mut plan = game.clone();
    plan.set_desktop_enabled(bulk.desktop);
    if bulk.verbose {
        let mut summary_out = Vec::new();
        writeln!(
            summary_out,
            "{}",
            summary(paths, plan.resolved(), plan.name())
        )?;
        out.extend(summary_out);
    }
    let art = ArtChoice {
        key: bulk.art.clone(),
        enabled: bulk.art.is_some(),
        term: plan.heroic().title.clone(),
    };
    let registered = apply_native(paths, bulk.source, &plan)?;
    super::report_playtime(&registered.playtime);
    let top = Pick::<NoInput, Vec<u8>>::Top(out);
    // A top match can't be canceled.
    let _ = fetch_art(
        paths,
        global,
        bulk.access,
        &registered.environment.name,
        &art,
        top,
        err,
    );
    super::finish_registered(paths, global, &registered)?;
    Ok(())
}

/// The `--list` lines for a native candidate.
pub(super) fn list_lines(candidate: &HeroicCandidate) {
    if let Some(exe) = &candidate.executable {
        println!("  exe: {}", exe.display());
    }
    for issue in &candidate.issues {
        print_issue("  issue", issue);
    }
}

#[cfg(test)]
mod tests {
    use crate::test_script::write_script;
    use std::cell::Cell;
    use std::fs;

    use std::path::Path;

    use wineo_core::steamgriddb::{Key, KeySource};

    use wineo_core::heroic_import::{Game, LibraryOptions, plan_library};

    use super::*;
    use crate::commands::art::fake::Fake;
    use crate::commands::import_heroic::test_access::access;

    /// A Heroic library holding one native sideload, Dusklight, whose script
    /// is in the home directory `root`.
    fn dusklight(root: &Path) -> (Paths, HeroicSource) {
        let heroic = root.join("heroic");
        fs::create_dir_all(heroic.join("sideload_apps")).unwrap();
        let exe = root.join("games/dusklight");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        write_script(&exe, "#!/bin/sh\n");
        fs::write(
            heroic.join("sideload_apps/library.json"),
            format!(
                r#"{{"games":[{{"app_name":"dusk","title":"Alpha","runner":"sideload","install":{{"platform":"linux","executable":"{}"}}}}]}}"#,
                exe.display()
            ),
        )
        .unwrap();
        let home = root.to_owned();
        let paths =
            Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
                .unwrap();
        let source = HeroicSource::from_config_dir_with_home(heroic, root.to_owned());
        (paths, source)
    }

    #[test]
    fn native_summary_shows_heroic_hook_paths_as_stored() {
        let root = tempfile::tempdir().unwrap();
        let (paths, source) = dusklight(root.path());
        fs::create_dir_all(source.config_dir().join("GamesConfig")).unwrap();
        fs::write(
            source.config_dir().join("GamesConfig/dusk.json"),
            serde_json::json!({
                "dusk": {
                    "beforeLaunchScriptPath": "/Heroic scripts/before 'game'.sh",
                    "afterLaunchScriptPath": "/Heroic scripts/after.sh",
                }
            })
            .to_string(),
        )
        .unwrap();

        let imported = source.inspect_native("dusk").unwrap();
        let printed = summary(&paths, &imported, "dusklight");

        assert!(printed.contains("before hook  /Heroic scripts/before 'game'.sh"));
        assert!(printed.contains("after hook   /Heroic scripts/after.sh"));
    }

    #[test]
    fn a_native_import_fetches_art_before_its_desktop_entry() {
        let root = tempfile::tempdir().unwrap();
        let (paths, source) = dusklight(root.path());
        let global = GlobalConfig::default();
        let mut fake = Fake::default();
        fake.search("Alpha", true);
        fake.icon_only();
        let lookups = Cell::new(0);
        let access = access(true, &lookups, fake);

        let listing = wineo_core::heroic_import::listing(&paths, &global, &source).unwrap();
        let library =
            plan_library(&paths, &global, &listing, None, &LibraryOptions::default()).unwrap();
        let mut ready = library.into_ready();
        assert_eq!(ready.len(), 1);
        let Game::Native(game) = ready.remove(0) else {
            panic!("expected a native game");
        };
        let bulk = Bulk {
            paths: &paths,
            global: &global,
            access: &access,
            source: &source,
            desktop: true,
            art: Some(Key {
                value: "k".into(),
                source: KeySource::Variable,
            }),
            verbose: false,
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        import_one(&bulk, &game, &mut out, &mut err).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.starts_with(
                "alpha: art from SteamGridDB game 7, Hades (2020)\nicon: fetched SteamGridDB 1\n"
            ),
            "{out}"
        );
        assert_eq!(String::from_utf8(err).unwrap(), "");
        assert!(paths.art_dir("alpha").join("icon.png").exists());
        let entry =
            fs::read_to_string(paths.applications_dir().join("wineo-alpha-game.desktop")).unwrap();
        assert!(entry.contains("\nX-Wineo-Icon=art:"), "{entry}");
    }
}
