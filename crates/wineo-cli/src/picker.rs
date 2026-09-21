//! Choosing and editing an environment's targets, shared by `setup` and the
//! guided `new`. Everything here only stages changes in a `Batch`, and the
//! caller reviews and writes them.

use std::io::Write;
use std::path::{Path, PathBuf};

use wineo_core::name;
use wineo_core::paths::Paths;
use wineo_core::programs::{self, Program, Source};
use wineo_core::targets::{self, Batch, Change, Spec};
use wineo_core::winpath;

use crate::prompt::{Choice, Input, Picked, Prompt, Reply, numbered};

/// Why `answer` can't be a name, and one that could be, or None when it can.
pub fn name_problem(answer: &str) -> Option<String> {
    if name::is_valid(answer) {
        return None;
    }
    if answer.is_empty() {
        return Some("Enter a name".to_owned());
    }
    let mut found: Vec<String> = Vec::new();
    for char in answer.chars() {
        if char.is_ascii_alphanumeric() || char == '-' || char == '_' {
            continue;
        }
        let shown = if char == ' ' {
            "a space".to_owned()
        } else {
            format!("`{char}`")
        };
        if !found.contains(&shown) {
            found.push(shown);
        }
    }
    let mut problem = format!(
        "`{answer}` has {}. Names go into commands, file names, and config keys, so they can only use letters, digits, `-`, and `_`",
        found.join(" and ")
    );
    let mut fixed = String::new();
    for char in answer.chars() {
        if char.is_ascii_alphanumeric() || char == '-' || char == '_' {
            fixed.push(char);
        } else if !fixed.is_empty() && !fixed.ends_with('-') {
            fixed.push('-');
        }
    }
    let fixed = fixed.trim_end_matches('-');
    if name::is_valid(fixed) {
        problem.push_str(&format!(". Try `{fixed}`"));
    }
    Some(problem)
}

/// A path as typed or pasted, which may come wrapped in quotes or with the
/// backslash a shell puts before a space. A Windows path keeps its
/// backslashes, since those are its separators.
pub fn clean_path(answer: &str) -> String {
    let answer = answer.trim();
    let unquoted = ['"', '\'']
        .iter()
        .find_map(|quote| answer.strip_prefix(*quote)?.strip_suffix(*quote))
        .unwrap_or(answer);
    if winpath::is_windows_path(unquoted) {
        unquoted.to_owned()
    } else {
        unquoted.replace("\\ ", " ")
    }
}

/// Where targets are picked from.
pub struct Place<'a> {
    pub paths: &'a Paths,
    /// The environment's name.
    pub name: &'a str,
    /// The wine prefix itself, which is `pfx/` inside a Proton environment's
    /// prefix. None when there is no prefix to look in.
    pub wine_prefix: Option<PathBuf>,
}

#[derive(Clone)]
enum Pick {
    Program(Program),
    All,
    Path,
    Done,
}

/// Adds targets from what the prefix has installed, or from a path, until
/// the user is done. Back means leave without adding anything more.
pub fn pick<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    place: &Place,
    batch: &mut Batch,
) -> anyhow::Result<Reply<()>> {
    // Scanned the same way `wineo programs` does, so both list the same things.
    let listing = place
        .wine_prefix
        .as_deref()
        .and_then(|wine_prefix| programs::scan(wine_prefix).ok())
        .unwrap_or_default();
    for (shortcut, reason) in &listing.unreadable {
        prompt.say(format!("Skipped the shortcut {shortcut}: {reason}"))?;
    }
    let mut all = false;
    let mut added = false;
    loop {
        let shown = if all {
            listing.all()
        } else {
            listing.programs.clone()
        };
        let heading = if shown.is_empty() {
            format!("Nothing installed was found in {}.", place.name)
        } else {
            format!("Programs in {}:", place.name)
        };
        let mut choices = numbered(
            rows(&shown, batch)
                .into_iter()
                .zip(shown.iter().cloned().map(Pick::Program)),
        );
        if !all && !listing.filtered.is_empty() {
            choices.push(Choice::letter(
                'a',
                format!(
                    "show {} held back as noise, like uninstallers",
                    listing.filtered.len()
                ),
                Pick::All,
            ));
        }
        choices.push(Choice::letter('p', "enter a path", Pick::Path));
        choices.push(Choice::letter(
            's',
            if added { "done adding" } else { "skip for now" },
            Pick::Done,
        ));
        let label = if shown.is_empty() {
            "Choice"
        } else {
            "Add which, like 1 3"
        };
        let picked = match prompt.choose_many(&heading, label, &choices, None, true)? {
            Reply::Back => return Ok(Reply::Back),
            Reply::Value(Picked::One(Pick::Done)) => return Ok(Reply::Value(())),
            Reply::Value(Picked::One(Pick::All)) => {
                all = true;
                continue;
            }
            Reply::Value(Picked::One(Pick::Path)) => match ask_path(prompt, place)? {
                Reply::Value(program) => vec![program],
                Reply::Back => continue,
            },
            Reply::Value(Picked::Many(picked)) => picked
                .into_iter()
                .filter_map(|pick| match pick {
                    Pick::Program(program) => Some(program),
                    _ => None,
                })
                .collect(),
            Reply::Value(Picked::One(Pick::Program(program))) => vec![program],
        };
        for program in picked {
            added |= add_program(prompt, place, batch, &program)?;
        }
    }
}

/// Program rows lined up in columns, noting what is already a target.
fn rows(programs: &[Program], batch: &Batch) -> Vec<String> {
    let names = programs.iter().map(|p| p.name.len()).max().unwrap_or(0);
    programs
        .iter()
        .map(|program| {
            let used = batch.using(&program.exe);
            let note = match used.as_slice() {
                [] => String::new(),
                [one] => format!("  (target {one})"),
                many => format!("  (targets {})", many.join(", ")),
            };
            format!(
                "{:names$}  {:8}  {}{note}",
                program.name,
                program.source.as_str(),
                program.exe
            )
        })
        .collect()
}

fn ask_path<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    place: &Place,
) -> anyhow::Result<Reply<Program>> {
    let reply = prompt.text(
        "Executable, as a Unix path or a Windows one",
        None,
        |answer| resolve_exe(place, answer),
    )?;
    Ok(match reply {
        Reply::Value(exe) => Reply::Value(Program {
            source: Source::Executable,
            name: file_stem(&exe),
            exe,
            lnk: None,
        }),
        Reply::Back => Reply::Back,
    })
}

/// Stages one program as a new target, after its card is accepted. Returns
/// whether it was added.
pub fn add_program<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    place: &Place,
    batch: &mut Batch,
    program: &Program,
) -> anyhow::Result<bool> {
    let used = batch.using(&program.exe);
    if !used.is_empty() {
        prompt.say(format!(
            "{} is already {}",
            program.exe,
            match used.as_slice() {
                [one] => format!("target {one}"),
                many => format!("targets {}", many.join(", ")),
            }
        ))?;
        if prompt.confirm("Add it again as another target?", false)? != Reply::Value(true) {
            return Ok(false);
        }
    }
    let suggested = targets::suggest_name(&program.name, &program.exe);
    let target = targets::free_name(&suggested, |taken| batch.contains(taken));
    let mut notes = Vec::new();
    if target != suggested {
        notes.push(format!(
            "`{suggested}` is already a target, so this one is `{target}` unless you change it."
        ));
    }
    if program.source == Source::Shortcut {
        notes.push("Shortcut arguments aren't imported, so add any it needs.".to_owned());
    }
    let spec = Spec {
        exe: program.exe.clone(),
        args: Vec::new(),
        cwd: None,
        name: Some(program.name.clone()),
        desktop: true,
    };
    let card = Card {
        adding: Some(&program.name),
        notes,
    };
    match card.show(prompt, place, batch, target, spec, false)? {
        Reply::Value((target, mut spec, default)) => {
            // A name only titles a desktop entry, so none is kept without one.
            if !spec.desktop {
                spec.name = None;
            }
            batch.add(&target, spec)?;
            if default {
                batch.set_default(Some(&target));
            }
            Ok(true)
        }
        Reply::Back => Ok(false),
    }
}

/// Edits one of the batch's targets.
pub fn edit<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    place: &Place,
    batch: &mut Batch,
) -> anyhow::Result<Reply<()>> {
    let Reply::Value(target) = which(prompt, place, batch, "Edit which target?")? else {
        return Ok(Reply::Back);
    };
    let spec = batch
        .targets()
        .remove(&target)
        .expect("picked from the batch");
    let default = batch.default().as_deref() == Some(target.as_str());
    let card = Card {
        adding: None,
        notes: Vec::new(),
    };
    match card.show(prompt, place, batch, target.clone(), spec, default)? {
        Reply::Value((_, spec, make_default)) => {
            batch.edit(&target, spec);
            if make_default && !default {
                batch.set_default(Some(&target));
            }
            Ok(Reply::Value(()))
        }
        Reply::Back => Ok(Reply::Back),
    }
}

/// Removes one of the batch's targets, after showing what goes with it.
pub fn remove<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    place: &Place,
    batch: &mut Batch,
) -> anyhow::Result<Reply<()>> {
    let Reply::Value(target) = which(prompt, place, batch, "Remove which target?")? else {
        return Ok(Reply::Back);
    };
    let spec = batch
        .targets()
        .remove(&target)
        .expect("picked from the batch");
    let mut heading = format!("Remove {target}?\n  exe      {}\n", spec.exe);
    if spec.desktop {
        heading.push_str(&format!(
            "  desktop  its entry \"{}\" goes too\n",
            desktop_name(&target, &spec)
        ));
    }
    heading.push_str("The program's files and the prefix are left alone.");
    let choices = numbered([("Remove it".to_owned(), ())]);
    if prompt.choose(&heading, "Choice", &choices, Some("1"), true)? == Reply::Back {
        return Ok(Reply::Back);
    }
    let was_default = batch.default().as_deref() == Some(target.as_str());
    batch.remove(&target);
    if was_default && batch.targets().len() > 1 {
        let heading = format!(
            "{target} was the default. Which target should `wineo start {}` launch now?",
            place.name
        );
        // Going back here still leaves the removal staged, with no default
        // yet, which finishing asks about again.
        choose_default(prompt, batch, &heading)?;
    }
    Ok(Reply::Value(()))
}

/// Asks which target `start <name>` launches, and marks it.
pub fn choose_default<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    batch: &mut Batch,
    heading: &str,
) -> anyhow::Result<Reply<()>> {
    let targets = batch.targets();
    let width = targets.keys().map(String::len).max().unwrap_or(0);
    let choices = numbered(
        targets
            .iter()
            .map(|(target, spec)| (format!("{target:width$}  {}", spec.exe), target.clone())),
    );
    match prompt.choose(heading, "Default", &choices, None, true)? {
        Reply::Value(target) => {
            batch.set_default(Some(&target));
            Ok(Reply::Value(()))
        }
        Reply::Back => Ok(Reply::Back),
    }
}

fn which<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    place: &Place,
    batch: &Batch,
    heading: &str,
) -> anyhow::Result<Reply<String>> {
    let targets = batch.targets();
    if targets.is_empty() {
        prompt.say(format!("{} has no targets yet.", place.name))?;
        return Ok(Reply::Back);
    }
    let width = targets.keys().map(String::len).max().unwrap_or(0);
    let choices = numbered(
        targets
            .iter()
            .map(|(target, spec)| (format!("{target:width$}  {}", spec.exe), target.clone())),
    );
    prompt.choose(heading, "Target", &choices, None, true)
}

/// One target's settings with its suggestions filled in, accepted as they
/// are or changed a field at a time.
struct Card<'a> {
    /// The program's label when this adds a target, None when it edits one.
    adding: Option<&'a str>,
    notes: Vec<String>,
}

#[derive(Clone, Copy)]
enum Field {
    Accept,
    Name,
    Exe,
    Args,
    Cwd,
    Desktop,
    Default,
}

impl Card<'_> {
    fn show<I: Input, W: Write>(
        &self,
        prompt: &mut Prompt<I, W>,
        place: &Place,
        batch: &Batch,
        mut target: String,
        mut spec: Spec,
        mut default: bool,
    ) -> anyhow::Result<Reply<(String, Spec, bool)>> {
        let original = target.clone();
        // Whether there is anything else to be the default instead of.
        let others = batch.targets().keys().any(|other| *other != original);
        loop {
            let mut heading = match self.adding {
                Some(label) => format!("Adding \"{label}\":\n  target       {target}\n"),
                None => format!("Editing {target}:\n"),
            };
            heading.push_str(&describe(&target, &spec));
            if others {
                heading.push_str(&format!(
                    "\n  default      {}",
                    if default { "yes" } else { "no" }
                ));
            }
            for note in &self.notes {
                heading.push('\n');
                heading.push_str(note);
            }
            let mut fields = vec![(
                if self.adding.is_some() {
                    "Add it"
                } else {
                    "Done"
                },
                Field::Accept,
            )];
            if self.adding.is_some() {
                fields.push(("Change the target name", Field::Name));
            } else {
                fields.push(("Change the executable", Field::Exe));
            }
            fields.push(("Change the arguments", Field::Args));
            fields.push(("Change the working directory", Field::Cwd));
            fields.push(("Change the desktop entry", Field::Desktop));
            if others && !default {
                fields.push(("Make it the default", Field::Default));
            }
            let choices = numbered(
                fields
                    .into_iter()
                    .map(|(label, field)| (label.to_owned(), field)),
            );
            let field = match prompt.choose(&heading, "Choice", &choices, Some("1"), true)? {
                Reply::Value(field) => field,
                Reply::Back => return Ok(Reply::Back),
            };
            match field {
                Field::Accept => return Ok(Reply::Value((target, spec, default))),
                Field::Name => {
                    let reply = prompt.text("Target name", Some(&target), |answer| {
                        if let Some(problem) = name_problem(answer) {
                            Err(problem)
                        } else if batch.contains(answer) {
                            Err(format!("There is already a target called `{answer}`"))
                        } else {
                            Ok(answer.to_owned())
                        }
                    })?;
                    if let Reply::Value(name) = reply {
                        target = name;
                    }
                }
                Field::Exe => {
                    let reply = prompt.text(
                        "Executable, as a Unix path or a Windows one",
                        Some(&spec.exe),
                        |answer| resolve_exe(place, answer),
                    )?;
                    if let Reply::Value(exe) = reply {
                        spec.exe = exe;
                    }
                }
                Field::Args => {
                    prompt.say(
                        "Arguments, one per line, and an empty line to finish. They replace the ones above.",
                    )?;
                    if let Reply::Value(args) = prompt.list("argument")? {
                        spec.args = args;
                    }
                }
                Field::Cwd => {
                    let reply = prompt.text(
                        "Working directory, or empty for the executable's folder",
                        None,
                        |answer| resolve_cwd(place, answer),
                    )?;
                    if let Reply::Value(cwd) = reply {
                        spec.cwd = cwd;
                    }
                }
                Field::Desktop => {
                    if let Reply::Value(wanted) = prompt.confirm("Desktop entry?", spec.desktop)? {
                        if wanted {
                            let current = desktop_name(&target, &spec);
                            let reply =
                                prompt.text("Desktop entry name", Some(&current), |answer| {
                                    if answer.is_empty() {
                                        Err("Enter a name".to_owned())
                                    } else {
                                        Ok(answer.to_owned())
                                    }
                                })?;
                            if let Reply::Value(name) = reply {
                                spec.desktop = true;
                                spec.name = Some(name);
                            }
                        } else {
                            spec.desktop = false;
                        }
                    }
                }
                Field::Default => default = true,
            }
        }
    }
}

/// The lines of a card that describe a target's settings.
fn describe(target: &str, spec: &Spec) -> String {
    format!(
        "  exe          {}\n  arguments    {}\n  working dir  {}\n  desktop      {}",
        spec.exe,
        show_args(&spec.args),
        spec.cwd.as_deref().unwrap_or("the executable's folder"),
        show_desktop(target, spec),
    )
}

fn show_args(args: &[String]) -> String {
    if args.is_empty() {
        return "none".to_owned();
    }
    args.iter()
        .map(|arg| {
            if arg.is_empty() || arg.contains(char::is_whitespace) {
                format!("\"{arg}\"")
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn show_desktop(target: &str, spec: &Spec) -> String {
    if spec.desktop {
        format!("yes, named \"{}\"", desktop_name(target, spec))
    } else {
        "no".to_owned()
    }
}

fn desktop_name(target: &str, spec: &Spec) -> String {
    spec.name.clone().unwrap_or_else(|| target.to_owned())
}

/// The review of a batch, one line per change.
pub fn review(batch: &Batch) -> Vec<String> {
    let summary = batch.summary();
    let width = summary
        .iter()
        .filter_map(|change| match change {
            Change::Added { target, .. }
            | Change::Edited { target, .. }
            | Change::Removed { target, .. } => Some(target.len()),
            Change::Default { .. } => None,
        })
        .max()
        .unwrap_or(0);
    let mut lines = Vec::new();
    for change in summary {
        match change {
            Change::Added { target, spec } => {
                let mut line = format!("  + {target:width$}  {}", spec.exe);
                if !spec.args.is_empty() {
                    line.push_str(&format!(" {}", show_args(&spec.args)));
                }
                if spec.desktop {
                    line.push_str(&format!(
                        ", desktop entry \"{}\"",
                        desktop_name(&target, &spec)
                    ));
                }
                lines.push(line);
            }
            Change::Edited {
                target,
                before,
                after,
            } => {
                let mut fields = Vec::new();
                if before.exe != after.exe {
                    fields.push(("exe", before.exe.clone(), after.exe.clone()));
                }
                if before.args != after.args {
                    fields.push(("arguments", show_args(&before.args), show_args(&after.args)));
                }
                if before.cwd != after.cwd {
                    let show = |cwd: &Option<String>| {
                        cwd.clone()
                            .unwrap_or_else(|| "the executable's folder".to_owned())
                    };
                    fields.push(("working dir", show(&before.cwd), show(&after.cwd)));
                }
                if before.desktop != after.desktop || before.name != after.name {
                    fields.push((
                        "desktop",
                        show_desktop(&target, &before),
                        show_desktop(&target, &after),
                    ));
                }
                for (field, from, to) in fields {
                    lines.push(format!("  ~ {target:width$}  {field}: {from} -> {to}"));
                }
            }
            Change::Removed { target, spec } => {
                let entry = if spec.desktop {
                    ", and its desktop entry"
                } else {
                    ""
                };
                lines.push(format!("  - {target:width$}  {}{entry}", spec.exe));
            }
            Change::Default { before, after } => lines.push(format!(
                "  default: {} (was {})",
                after.as_deref().unwrap_or("none"),
                before.as_deref().unwrap_or("none")
            )),
        }
    }
    lines
}

/// An executable as a target's `exe` holds it: a Windows path as typed, a
/// Unix path inside `drive_c` as its `C:` path, and any other Unix path
/// made absolute, with the home directory as `~`.
fn resolve_exe(place: &Place, answer: &str) -> Result<String, String> {
    let answer = &clean_path(answer);
    if answer.is_empty() {
        return Err("Enter the path to the program".to_owned());
    }
    let missing = || format!("`{answer}` does not exist");
    if winpath::is_windows_path(answer) {
        let wine_prefix = place.wine_prefix.as_deref().ok_or_else(missing)?;
        return match winpath::to_unix(wine_prefix, answer) {
            Some(unix) if unix.is_file() => Ok(answer.to_owned()),
            _ => Err(missing()),
        };
    }
    let unix = place
        .paths
        .absolute(answer)
        .map_err(|err| err.to_string())?;
    if !unix.is_file() {
        return Err(missing());
    }
    Ok(config_path(place, &unix))
}

fn resolve_cwd(place: &Place, answer: &str) -> Result<Option<String>, String> {
    let answer = &clean_path(answer);
    if answer.is_empty() {
        return Ok(None);
    }
    let missing = || format!("`{answer}` is not a directory");
    if winpath::is_windows_path(answer) {
        let wine_prefix = place.wine_prefix.as_deref().ok_or_else(missing)?;
        return match winpath::to_unix(wine_prefix, answer) {
            Some(unix) if unix.is_dir() => Ok(Some(answer.to_owned())),
            _ => Err(missing()),
        };
    }
    let unix = place
        .paths
        .absolute(answer)
        .map_err(|err| err.to_string())?;
    if !unix.is_dir() {
        return Err(missing());
    }
    Ok(Some(config_path(place, &unix)))
}

fn config_path(place: &Place, unix: &Path) -> String {
    place
        .wine_prefix
        .as_deref()
        .and_then(|wine_prefix| winpath::from_unix(wine_prefix, unix))
        .or_else(|| place.paths.contract(unix).ok())
        .unwrap_or_else(|| unix.display().to_string())
}

fn file_stem(path: &str) -> String {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    file.rsplit_once('.')
        .map_or(file, |(stem, _)| stem)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::symlink;

    use wineo_core::config::EnvironmentConfig;

    use super::*;
    use crate::prompt::Script;

    struct Fixture {
        root: tempfile::TempDir,
        paths: Paths,
        wine_prefix: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().canonicalize().unwrap();
            let lookup = home.clone();
            let paths = Paths::from_lookup(move |key| {
                (key == "HOME").then(|| OsString::from(lookup.as_os_str()))
            })
            .unwrap();
            let wine_prefix = home.join("prefix");
            fs::create_dir_all(wine_prefix.join("dosdevices")).unwrap();
            symlink("../drive_c", wine_prefix.join("dosdevices/c:")).unwrap();
            for exe in [
                "drive_c/Games/Hades/Hades.exe",
                "drive_c/Games/Hades/Config.exe",
                "drive_c/Games/Hades/unins000.exe",
            ] {
                let path = wine_prefix.join(exe);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, b"").unwrap();
            }
            Self {
                root,
                paths,
                wine_prefix,
            }
        }

        fn place(&self) -> Place<'_> {
            Place {
                paths: &self.paths,
                name: "hades",
                wine_prefix: Some(self.wine_prefix.clone()),
            }
        }
    }

    fn batch(targets: &str) -> Batch {
        let text = format!("prefix = \"/p\"\nrunner = \"wine\"\narch = \"win64\"\n{targets}");
        Batch::new(
            EnvironmentConfig::parse(Path::new("/x.toml"), &text)
                .unwrap()
                .targets,
        )
    }

    fn run<T>(
        answers: &[&str],
        flow: impl FnOnce(&mut Prompt<Script, Vec<u8>>) -> anyhow::Result<T>,
    ) -> (anyhow::Result<T>, String) {
        let mut prompt = Prompt::new(Script::new(answers), Vec::new());
        let result = flow(&mut prompt);
        let printed = String::from_utf8_lossy(prompt.output()).into_owned();
        (result, printed)
    }

    #[test]
    fn picking_programs_adds_them_with_suggested_settings() {
        let f = Fixture::new();
        let mut batch = batch("");
        let (result, printed) = run(&["1 2", "", "", "s"], |prompt| {
            pick(prompt, &f.place(), &mut batch)
        });
        result.unwrap();
        // Noise is held back, and named in the menu rather than listed.
        assert!(
            printed.contains("a) show 1 held back as noise"),
            "{printed}"
        );
        assert!(!printed.contains("unins000"), "{printed}");
        let targets = batch.targets();
        assert_eq!(targets.keys().collect::<Vec<_>>(), ["config", "hades"]);
        assert_eq!(targets["hades"].exe, r"C:\Games\Hades\Hades.exe");
        assert!(targets["hades"].desktop);
        assert_eq!(targets["hades"].name.as_deref(), Some("Hades"));
        assert!(targets["hades"].args.is_empty());
        assert_eq!(targets["hades"].cwd, None);
    }

    #[test]
    fn a_card_changes_one_field_at_a_time() {
        let f = Fixture::new();
        let mut batch = batch("");
        let answers = [
            "1",
            // Rename it, then give it two arguments, one with spaces.
            "2",
            "game",
            "3",
            "-config",
            r"C:\My Games\settings.ini",
            "",
            // Working directory as a Unix path inside drive_c.
            "4",
            &format!("{}/drive_c/Games", f.wine_prefix.display()),
            // No desktop entry after all.
            "5",
            "n",
            "1",
            "s",
        ];
        let (result, printed) = run(&answers, |prompt| pick(prompt, &f.place(), &mut batch));
        result.unwrap();
        let targets = batch.targets();
        let game = &targets["game"];
        assert_eq!(game.args, ["-config", r"C:\My Games\settings.ini"]);
        assert_eq!(game.cwd.as_deref(), Some(r"C:\Games"));
        assert!(!game.desktop);
        assert_eq!(game.name, None);
        assert!(
            printed.contains(r#"  arguments    -config "C:\My Games\settings.ini""#),
            "{printed}"
        );
    }

    #[test]
    fn an_executable_in_use_is_not_added_again_without_asking() {
        let f = Fixture::new();
        let mut batch = batch("[targets.hades]\nexe = 'C:\\Games\\Hades\\Hades.exe'\n");
        // Say no to the duplicate, then yes, which suggests a free name.
        let (result, printed) = run(&["2", "", "2", "y", "", "s"], |prompt| {
            pick(prompt, &f.place(), &mut batch)
        });
        result.unwrap();
        assert!(printed.contains("(target hades)"), "{printed}");
        assert!(
            printed.contains(r"C:\Games\Hades\Hades.exe is already target hades"),
            "{printed}"
        );
        assert!(
            printed.contains("`hades` is already a target, so this one is `hades-2`"),
            "{printed}"
        );
        assert_eq!(
            batch.targets().keys().collect::<Vec<_>>(),
            ["hades", "hades-2"]
        );
    }

    #[test]
    fn a_path_can_be_entered_with_spaces_and_outside_the_prefix() {
        let f = Fixture::new();
        let outside = f
            .root
            .path()
            .canonicalize()
            .unwrap()
            .join("My Apps/tool.exe");
        fs::create_dir_all(outside.parent().unwrap()).unwrap();
        fs::write(&outside, b"").unwrap();
        let mut batch = batch("");
        let answers = [
            "p",
            "/nowhere/nothing.exe",
            outside.to_str().unwrap(),
            "",
            "s",
        ];
        let (result, printed) = run(&answers, |prompt| pick(prompt, &f.place(), &mut batch));
        result.unwrap();
        assert!(
            printed.contains("`/nowhere/nothing.exe` does not exist"),
            "{printed}"
        );
        // Stored the way `wineo new` stores paths under the home directory.
        assert_eq!(batch.targets()["tool"].exe, "~/My Apps/tool.exe");
    }

    #[test]
    fn an_empty_prefix_still_takes_a_path() {
        let f = Fixture::new();
        let place = Place {
            wine_prefix: None,
            ..f.place()
        };
        let mut batch = batch("");
        let (result, printed) = run(&["s"], |prompt| pick(prompt, &place, &mut batch));
        result.unwrap();
        assert!(printed.contains("Nothing installed was found in hades."));
        assert!(printed.contains("  p) enter a path\n"), "{printed}");
    }

    #[test]
    fn removing_the_default_asks_for_another() {
        let f = Fixture::new();
        let mut batch = batch(
            "[targets.a]\nexe = 'a'\ndefault = true\ndesktop = true\n[targets.b]\nexe = 'b'\n[targets.c]\nexe = 'c'\n",
        );
        let (result, printed) = run(&["1", "1", "2"], |prompt| {
            remove(prompt, &f.place(), &mut batch)
        });
        result.unwrap();
        assert!(printed.contains("its entry \"a\" goes too"), "{printed}");
        assert!(
            printed.contains("The program's files and the prefix are left alone."),
            "{printed}"
        );
        assert_eq!(batch.default().as_deref(), Some("c"));
        assert_eq!(
            review(&batch),
            ["  - a  a, and its desktop entry", "  default: c (was a)"]
        );
    }

    #[test]
    fn editing_keeps_the_desktop_choice_and_can_move_the_default() {
        let f = Fixture::new();
        let mut batch = batch(
            "[targets.a]\nexe = 'a'\ndefault = true\n[targets.b]\nexe = 'b'\ndesktop = true\nname = \"Bee\"\n",
        );
        let (result, printed) = run(&["2", "3", "-x", "", "6", "1"], |prompt| {
            edit(prompt, &f.place(), &mut batch)
        });
        result.unwrap();
        assert!(
            printed.contains("  desktop      yes, named \"Bee\""),
            "{printed}"
        );
        let targets: BTreeMap<_, _> = batch.targets();
        assert!(targets["b"].desktop);
        assert_eq!(targets["b"].args, ["-x"]);
        assert_eq!(batch.default().as_deref(), Some("b"));
        assert_eq!(
            review(&batch),
            ["  ~ b  arguments: none -> -x", "  default: b (was a)"]
        );
    }

    #[test]
    fn back_leaves_the_batch_as_it_was() {
        let f = Fixture::new();
        let mut batch = batch("[targets.a]\nexe = 'a'\n");
        let (result, _) = run(&["1", "3", "-x", "", "b"], |prompt| {
            edit(prompt, &f.place(), &mut batch)
        });
        assert_eq!(result.unwrap(), Reply::Back);
        assert!(batch.is_empty());
    }

    #[test]
    fn a_bad_name_says_what_is_wrong_and_what_would_work() {
        assert_eq!(name_problem("Stellar-Blade"), None);
        assert_eq!(
            name_problem("Stellar Blade").unwrap(),
            "`Stellar Blade` has a space. Names go into commands, file names, and config keys, so they can only use letters, digits, `-`, and `_`. Try `Stellar-Blade`"
        );
        let problem = name_problem("Hades: II!").unwrap();
        assert!(
            problem.starts_with("`Hades: II!` has `:` and a space and `!`."),
            "{problem}"
        );
        assert!(problem.ends_with("Try `Hades-II`"), "{problem}");
        assert!(!name_problem("游戏").unwrap().contains("Try"));
    }

    #[test]
    fn a_pasted_path_loses_its_quotes_and_escapes() {
        assert_eq!(
            clean_path(r#" "~/Games/Stellar Blade/SB.exe" "#),
            "~/Games/Stellar Blade/SB.exe"
        );
        assert_eq!(clean_path("'/games/My Game/g.exe'"), "/games/My Game/g.exe");
        assert_eq!(
            clean_path(r"~/Games/Stellar\ Blade/SB.exe"),
            "~/Games/Stellar Blade/SB.exe"
        );
        // A Windows path's backslashes are its separators.
        assert_eq!(
            clean_path(r#""C:\Games\ SB\SB.exe""#),
            r"C:\Games\ SB\SB.exe"
        );
        assert_eq!(clean_path(r#""unbalanced"#), r#""unbalanced"#);
    }

    #[test]
    fn stems_come_from_either_kind_of_path() {
        assert_eq!(file_stem(r"C:\Games\Hades\Hades.exe"), "Hades");
        assert_eq!(file_stem("~/My Apps/tool.exe"), "tool");
        assert_eq!(file_stem("run"), "run");
    }
}
