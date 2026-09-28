//! Importing a native sideloaded Heroic game as a native environment. There
//! is no prefix or runner, so this skips everything about them and ignores
//! the Wine settings Heroic keeps for native games too.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use wineo_core::config::{GlobalConfig, Target};
use wineo_core::error::Error;
use wineo_core::heroic::{
    HeroicCandidate, HeroicGameKind, HeroicIssue, HeroicIssueKind, HeroicListing,
    HeroicNativeImport, HeroicSource,
};
use wineo_core::heroic_import::{DesktopEntry, redact_env_value};
use wineo_core::import::{self, ImportedSettings};
use wineo_core::launch;
use wineo_core::paths::Paths;

use super::{
    Args, ArtChoice, Block, Bulk, BulkRow, OutputOnly, RowStatus, display_args, edit_art,
    edit_desktop, fetch_art, gamescope_display, on_off, print_issue, render_blocks, review_art,
};
use crate::commands::art::{Access, Pick};
use crate::commands::desktop;
use crate::commands::import::finish_guided;
use crate::picker;
use crate::prompt::{self, Canceled, Input, Prompt, Reply, Terminal, numbered};
use crate::style;

/// The target a native import gets.
const TARGET: &str = "game";

/// Heroic's list, with a native game whose exe can't be run marked as not
/// importable, the same way a launch would refuse it.
pub(super) fn listing(
    paths: &Paths,
    global: &GlobalConfig,
    source: &HeroicSource,
) -> anyhow::Result<HeroicListing> {
    let mut listing = source.list_candidates()?;
    for candidate in &mut listing.candidates {
        if candidate.kind != HeroicGameKind::Native || filter_issue(candidate).is_some() {
            continue;
        }
        let Some(exe) = candidate.executable.clone() else {
            continue;
        };
        let name = super::default_name_for(&candidate.title, Some(&exe));
        if let Some(message) = exe_problem(paths, global, &name, &BTreeMap::new(), &exe) {
            candidate.issues.push(HeroicIssue {
                kind: HeroicIssueKind::Missing,
                field: Some("executable".to_owned()),
                message,
            });
        }
    }
    Ok(listing)
}

/// Why a native game's exe can't be run, checked the way a launch resolves it.
fn exe_problem(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    env: &BTreeMap<String, String>,
    exe: &Path,
) -> Option<String> {
    match launch::check_native_exe(paths, global, name, env, &exe.to_string_lossy()) {
        Ok(_) => None,
        Err(Error::ExeNotFound(_) | Error::NotOnPath(_)) => {
            Some(format!("executable not found: {}", exe.display()))
        }
        Err(err) => Some(err.to_string()),
    }
}

/// What keeps a native candidate from being imported. Unsupported settings,
/// like wrappers, are only reported, as they are for Windows games.
pub(super) fn filter_issue(candidate: &HeroicCandidate) -> Option<&HeroicIssue> {
    candidate.issues.iter().find(|issue| {
        (matches!(
            issue.kind,
            HeroicIssueKind::Missing | HeroicIssueKind::Invalid
        ) && issue.field.as_deref() != Some("launcherArgs"))
            || (issue.kind == HeroicIssueKind::Unsupported
                && matches!(issue.field.as_deref(), Some("platform") | Some("runner")))
    })
}

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

/// A launcher argument string that can't be parsed is refused rather than
/// guessed at, and so is anything else Heroic stored wrong.
fn reject_invalid(imported: &HeroicNativeImport) -> anyhow::Result<()> {
    if let Some(issue) = imported
        .issues
        .iter()
        .find(|issue| issue.kind == HeroicIssueKind::Invalid)
    {
        anyhow::bail!("{}", issue.message);
    }
    Ok(())
}

fn ensure_unchanged(source: &HeroicSource, imported: &HeroicNativeImport) -> anyhow::Result<()> {
    anyhow::ensure!(
        source.unchanged(&imported.id, &imported.fingerprint)?,
        "Heroic settings changed while they were being reviewed; inspect the game again"
    );
    Ok(())
}

fn settings(imported: &HeroicNativeImport, desktop: &DesktopEntry) -> ImportedSettings {
    let target = Target {
        exe: imported.exe.to_string_lossy().into_owned(),
        args: imported.args.clone(),
        cwd: None,
        default: true,
        gamemode: None,
        wayland: None,
        mangohud: None,
        gamescope: None,
        desktop: desktop.enabled,
        name: desktop.enabled.then(|| desktop.name.clone()),
    };
    ImportedSettings {
        targets: BTreeMap::from([(TARGET.to_owned(), target)]),
        env: imported.env.clone(),
        gamemode: imported.gamemode,
        mangohud: imported.mangohud,
        gamescope: imported.gamescope.clone(),
        heroic_id: Some(imported.id.clone()),
        ..ImportedSettings::default()
    }
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

/// A one-line note for `--all` on what wasn't imported, if anything.
fn bulk_warning(imported: &HeroicNativeImport) -> Option<String> {
    let omitted = imported
        .issues
        .iter()
        .filter(|issue| issue.kind == HeroicIssueKind::Unsupported)
        .count();
    let mut warnings = Vec::new();
    if omitted > 0 {
        warnings.push(format!(
            "{omitted} unsupported setting{} omitted",
            if omitted == 1 { "" } else { "s" }
        ));
    }
    if imported.issues.iter().any(|issue| {
        matches!(
            issue.kind,
            HeroicIssueKind::NeedsAttention | HeroicIssueKind::Conflict
        )
    }) {
        warnings.push("settings need attention, see -v".to_owned());
    }
    (!warnings.is_empty()).then(|| warnings.join("; "))
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
    let imported = source.inspect_native(game)?;
    refuse_wine_flags(args, &imported.title)?;
    reject_invalid(&imported)?;
    if let Some(problem) = exe_problem(paths, global, &name, &imported.env, &imported.exe) {
        anyhow::bail!("{problem}");
    }
    let desktop_choice = DesktopEntry {
        enabled: args.desktop,
        name: imported.title.clone(),
    };
    let art = if args.art {
        let check = (access.check_key)();
        check.print_skip();
        ArtChoice::checked(check, &imported.title)
    } else {
        ArtChoice::off()
    };
    println!("{}", summary(paths, &imported, &name));
    ensure_unchanged(source, &imported)?;
    let environment = import::register_native(paths, &name, &settings(&imported, &desktop_choice))?;
    super::import_playtime(paths, source, &imported.id, &environment.name);
    let mut prompt = (art.enabled && prompt::interactive())
        .then(|| Terminal::open().ok())
        .flatten()
        .map(|input| Prompt::new(input, io::stdout()));
    let mut stdout = io::stdout();
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
        &mut io::stderr(),
    );
    drop(prompt);
    desktop::reconcile_native(paths, global, &environment)?;
    if fetched.is_err() {
        let name = &environment.name;
        eprintln!("Canceled. Kept {name}, continue with `wineo setup {name}`.");
        return Ok(ExitCode::from(130));
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
    let mut imported = source.inspect_native(&candidate.id)?;
    refuse_wine_flags(args, &imported.title)?;
    reject_invalid(&imported)?;
    let suggested = super::default_name_for(&imported.title, Some(&imported.exe));
    let mut name = match prompt.text(
        "Environment name",
        Some(args.name.as_deref().unwrap_or(&suggested)),
        |value| picker::name_problem(value).map_or_else(|| Ok(value.to_owned()), Err),
    )? {
        Reply::Value(value) => value,
        Reply::Back => return Ok(ExitCode::SUCCESS),
    };
    let mut desktop_choice = DesktopEntry {
        enabled: paths.portable_root().is_none(),
        name: imported.title.clone(),
    };
    let mut art = review_art(access, prompt, &imported.title)?;
    loop {
        let heading = format!("{}\n", summary(paths, &imported, &name));
        let mut items = vec![
            ("name", name.clone(), Field::Name),
            (
                "gamemode",
                on_off(imported.gamemode.unwrap_or(false)).to_owned(),
                Field::GameMode,
            ),
            (
                "desktop",
                if desktop_choice.enabled { "yes" } else { "no" }.to_owned(),
                Field::Desktop,
            ),
        ];
        if art.key.is_some() {
            items.push(("art", art.display(), Field::Art));
        }
        let width = items
            .iter()
            .map(|(label, _, _)| label.len())
            .max()
            .unwrap_or(0)
            + 2;
        let choices = numbered(
            items
                .into_iter()
                .map(|(label, value, field)| (format!("{label:width$}{value}"), field)),
        );
        match prompt.review(&heading, None, "import", &choices, true)? {
            Reply::Value(None) => break,
            Reply::Value(Some(Field::Name)) => {
                if let Reply::Value(value) =
                    prompt.text("Environment name", Some(&name), |value| {
                        picker::name_problem(value).map_or_else(|| Ok(value.to_owned()), Err)
                    })?
                {
                    name = value;
                }
            }
            Reply::Value(Some(Field::GameMode)) => {
                let current = imported.gamemode.unwrap_or(false);
                if let Reply::Value(value) = prompt.confirm("Enable GameMode?", current)? {
                    imported.gamemode = Some(value);
                }
            }
            Reply::Value(Some(Field::Desktop)) => {
                desktop_choice =
                    edit_desktop(paths, &imported.title, desktop_choice.clone(), prompt)?;
            }
            Reply::Value(Some(Field::Art)) => edit_art(&mut art, prompt)?,
            Reply::Back => return Err(Canceled.into()),
        }
    }
    if let Some(problem) = exe_problem(paths, global, &name, &imported.env, &imported.exe) {
        anyhow::bail!("{problem}");
    }
    ensure_unchanged(source, &imported)?;
    let environment = import::register_native(paths, &name, &settings(&imported, &desktop_choice))?;
    super::import_playtime(paths, source, &imported.id, &environment.name);
    let fetched = fetch_art(
        paths,
        global,
        access,
        &environment.name,
        &art,
        Pick::Ask(prompt),
        &mut io::stderr(),
    );
    desktop::reconcile_native(paths, global, &environment)?;
    if fetched.is_err() {
        prompt.say(format!(
            "Kept {}. Continue with `wineo setup {}`",
            environment.name, environment.name
        ))?;
        return Err(Canceled.into());
    }
    finish_guided(paths, global, &environment, prompt)
}

/// A native game `--all` will import.
pub(super) struct ReadyNative {
    pub name: String,
    imported: HeroicNativeImport,
}

impl ReadyNative {
    /// The Heroic game id.
    pub fn id(&self) -> &str {
        &self.imported.id
    }
}

/// Sorts one native candidate for `--all` into a row, and the game when it
/// is ready. `taken` says whether a name is already in use.
pub(super) fn classify(
    paths: &Paths,
    global: &GlobalConfig,
    source: &HeroicSource,
    candidate: &HeroicCandidate,
    taken: impl Fn(&str) -> bool,
) -> (BulkRow, Option<ReadyNative>) {
    let default_name = super::default_name_for(&candidate.title, candidate.executable.as_deref());
    let skip = |name: String, reason: String| BulkRow {
        name,
        title: candidate.title.clone(),
        status: RowStatus::Skipped(reason),
    };
    if let Some(issue) = filter_issue(candidate) {
        return (skip(default_name, issue.message.clone()), None);
    }
    let imported = match source.inspect_native(&candidate.id) {
        Ok(imported) => imported,
        Err(error) => return (skip(default_name, format!("{error:#}")), None),
    };
    if imported
        .issues
        .iter()
        .any(|issue| issue.field.as_deref() == Some("launcherArgs"))
    {
        return (
            skip(default_name, "invalid launcher arguments".to_owned()),
            None,
        );
    }
    if let Some(issue) = imported
        .issues
        .iter()
        .find(|issue| issue.kind == HeroicIssueKind::Invalid)
    {
        return (skip(default_name, issue.message.clone()), None);
    }
    let name = default_name;
    if taken(&name) {
        let reason = format!("name `{name}` is already taken");
        return (skip(name, reason), None);
    }
    if let Some(problem) = exe_problem(paths, global, &name, &imported.env, &imported.exe) {
        return (skip(name, problem), None);
    }
    let row = BulkRow {
        name: name.clone(),
        title: imported.title.clone(),
        status: RowStatus::ReadyNative {
            warn: bulk_warning(&imported),
        },
    };
    (row, Some(ReadyNative { name, imported }))
}

/// Imports one native game of `--all`, like `import_one` does a Windows one.
pub(super) fn import_one(
    bulk: &Bulk,
    game: &ReadyNative,
    out: &mut Vec<u8>,
    err: &mut Vec<u8>,
) -> anyhow::Result<()> {
    let (paths, global) = (bulk.paths, bulk.global);
    if bulk.verbose {
        let mut summary_out = Vec::new();
        writeln!(
            summary_out,
            "{}",
            summary(paths, &game.imported, &game.name)
        )?;
        out.extend(summary_out);
    }
    let desktop_choice = DesktopEntry {
        enabled: bulk.desktop,
        name: game.imported.title.clone(),
    };
    let art = ArtChoice {
        key: bulk.art.clone(),
        enabled: bulk.art.is_some(),
        term: game.imported.title.clone(),
    };
    ensure_unchanged(bulk.source, &game.imported)?;
    let environment = import::register_native(
        paths,
        &game.name,
        &settings(&game.imported, &desktop_choice),
    )?;
    super::import_playtime(paths, bulk.source, &game.imported.id, &environment.name);
    let top = Pick::<OutputOnly, Vec<u8>>::Top(out);
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
    desktop::reconcile_native(paths, global, &environment)?;
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
    use std::cell::Cell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use wineo_core::steamgriddb::{Key, KeySource};

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
        fs::write(&exe, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
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
    fn a_native_import_fetches_art_before_its_desktop_entry() {
        let root = tempfile::tempdir().unwrap();
        let (paths, source) = dusklight(root.path());
        let global = GlobalConfig::default();
        let mut fake = Fake::default();
        fake.search("Alpha", true);
        fake.icon_only();
        let lookups = Cell::new(0);
        let access = access(true, &lookups, fake);

        let listing = listing(&paths, &global, &source).unwrap();
        let (row, game) = classify(&paths, &global, &source, &listing.candidates[0], |_| false);
        assert!(matches!(row.status, RowStatus::ReadyNative { .. }));
        let game = game.unwrap();
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
