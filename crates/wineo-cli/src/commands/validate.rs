use std::io::{self, Write};
use std::process::ExitCode;

use serde::Serialize;
use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::paths::Paths;
use wineo_core::validate::{self, Fix, Level, Note, Problem, Report};

use super::{components, json};
use crate::cancel;
use crate::prompt::{self, Input, Prompt, Reply, Terminal, numbered};
use crate::style;
use crate::text;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to check, instead of everything
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: Option<String>,

    /// Apply every fix without asking
    #[arg(long)]
    fix: bool,

    /// Print the problems as JSON, without asking anything
    #[arg(long)]
    json: bool,
}

#[derive(Serialize)]
struct JsonOutput<'a> {
    problems: &'a [Problem],
    fixed: Vec<String>,
    errors: &'a [Failed],
}

pub fn run(paths: &Paths, args: Args, verbose: bool) -> anyhow::Result<ExitCode> {
    let check = || match &args.name {
        Some(name) => validate::check_environment(paths, name),
        None => validate::check_all(paths),
    };
    let report = check()?;
    let fixes = report.fixes();

    if args.json {
        let (applied, failed) = if args.fix {
            apply_all(paths, &fixes)
        } else {
            (Vec::new(), Vec::new())
        };
        let report = if applied.is_empty() { report } else { check()? };
        json::print(&JsonOutput {
            problems: &report.problems,
            fixed: applied.iter().map(Fix::label).collect(),
            errors: &failed,
        })?;
        return Ok(exit(&report, !failed.is_empty()));
    }

    let color = style::stdout_enabled();
    let mut out = crate::output::stdout();
    print_report(
        paths,
        &report,
        verbose,
        args.name.is_some(),
        color,
        &mut out,
    )?;
    if report.problems.is_empty() {
        writeln!(out, "{}", clean(&report, args.name.as_deref()))?;
        return Ok(ExitCode::SUCCESS);
    }
    writeln!(out)?;

    let chosen = if fixes.is_empty() {
        writeln!(out, "{}", summary(&report))?;
        return Ok(exit(&report, false));
    } else if args.fix {
        writeln!(out, "{}", summary(&report))?;
        fixes
    } else if prompt::interactive() {
        let mut prompt = Prompt::new(Terminal::open()?, crate::output::stdout()).with_color(color);
        match review(&mut prompt, &summary(&report), fixes) {
            Err(err) if cancel::prompted(&err) => {
                eprintln!("Canceled. Nothing was fixed.");
                return Ok(cancel::exit_code());
            }
            other => other?,
        }
    } else {
        writeln!(out, "{}", summary(&report))?;
        return Ok(exit(&report, false));
    };
    if chosen.is_empty() {
        writeln!(out, "Nothing was fixed.")?;
        return Ok(exit(&report, false));
    }

    let (applied, failed) = apply_all(paths, &chosen);
    for failed in &failed {
        eprintln!(
            "{}",
            style::error_stderr(format!("error: {} failed: {}", failed.fix, failed.message))
        );
    }
    let fixed = report
        .problems
        .iter()
        .filter(|problem| {
            problem
                .fix
                .as_ref()
                .is_some_and(|fix| applied.contains(fix))
        })
        .count();
    let after = check()?;
    writeln!(out, "{}", fixed_line(fixed, &after))?;
    Ok(exit(&after, !failed.is_empty()))
}

/// 1 when errors are left or a fix failed.
fn exit(report: &Report, failed: bool) -> ExitCode {
    if failed || report.errors() > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// A fix that failed, for `--json`.
#[derive(Serialize)]
struct Failed {
    fix: String,
    message: String,
}

/// Applies each fix on its own, so one failing leaves the rest to go on.
/// Returns the ones that worked and why the others didn't.
fn apply_all(paths: &Paths, fixes: &[Fix]) -> (Vec<Fix>, Vec<Failed>) {
    // The report these fixes came from already carries `global_config_invalid`
    // for a file that fell back to defaults, so only a refusal matters here.
    let global = GlobalConfig::load_or_default(paths).map(|loaded| loaded.config);
    let mut applied = Vec::new();
    let mut failed = Vec::new();
    for fix in fixes.iter().filter(|fix| fix.is_batch_safe()) {
        // Only winetricks honors the token here. A handler held across the
        // other fixes would swallow Ctrl+C instead of ending wineo.
        let cancellation = match fix {
            Fix::InstallWinetricks { .. } => match super::transfer::Cancellation::new() {
                Ok(cancellation) => Some(cancellation),
                Err(err) => {
                    failed.push(Failed {
                        fix: fix.label(),
                        message: paths.contract_text(&format!("{err:#}")),
                    });
                    continue;
                }
            },
            _ => None,
        };
        let token = cancellation
            .as_ref()
            .map_or_else(Token::new, |cancellation| cancellation.token.clone());
        let result = validate::apply(
            paths,
            fix,
            &token,
            wineo_core::winetricks::EnsureOutput::Stderr,
            super::winetricks::print_event,
            |component, version| {
                let global = global.as_ref().map_err(|err| anyhow::anyhow!("{err}"))?;
                components::download_build(paths, global, component, version)
            },
        );
        drop(cancellation);
        match result {
            Ok(notes) => {
                for note in notes {
                    match note {
                        Note::Warning(warning) => eprintln!("warning: {warning}"),
                    }
                }
                applied.push(fix.clone());
            }
            Err(err) => failed.push(Failed {
                fix: fix.label(),
                message: paths.contract_text(&format!("{err:#}")),
            }),
        }
    }
    (applied, failed)
}

/// The review from decision 2: Enter fixes everything listed, and numbers
/// take those fixes off the list.
fn review<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    heading: &str,
    mut fixes: Vec<Fix>,
) -> anyhow::Result<Vec<Fix>> {
    while !fixes.is_empty() {
        let choices = numbered(
            fixes
                .iter()
                .enumerate()
                .map(|(index, fix)| (fix.label(), index)),
        );
        match prompt.review_many(
            heading,
            Some("Numbers skip those fixes, like 1 3 4."),
            "fix them",
            &choices,
            false,
        )? {
            Reply::Value(None) => return Ok(fixes),
            Reply::Value(Some(skipped)) => {
                fixes = fixes
                    .into_iter()
                    .enumerate()
                    .filter(|(index, _)| !skipped.contains(index))
                    .map(|(_, fix)| fix)
                    .collect();
            }
            Reply::Back => {}
        }
    }
    Ok(fixes)
}

/// What a group of problems is listed under.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Group {
    Global,
    Template(String),
    Environment(String),
}

impl Group {
    fn of(problem: &Problem) -> Self {
        if let Some(template) = &problem.template {
            Group::Template(template.clone())
        } else if let Some(name) = problem.environment.as_ref().or(problem.stray.as_ref()) {
            Group::Environment(name.clone())
        } else {
            Group::Global
        }
    }

    fn heading(&self, paths: &Paths) -> String {
        let shown = |path: std::path::PathBuf| {
            paths
                .contract(&path)
                .unwrap_or_else(|_| path.display().to_string())
        };
        match self {
            Group::Global => shown(paths.global_config()),
            Group::Template(name) => shown(paths.templates_dir().join(format!("{name}.toml"))),
            Group::Environment(name) => name.clone(),
        }
    }
}

/// Problems grouped under what they are in. With `verbose`, everything that
/// was checked is listed, the clean ones with `ok`. `named` is a run on one
/// environment, which leaves the global config and templates out.
fn print_report(
    paths: &Paths,
    report: &Report,
    verbose: bool,
    named: bool,
    color: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    let mut groups: Vec<Group> = Vec::new();
    if verbose {
        if !named {
            groups.push(Group::Global);
        }
        groups.extend(report.templates.iter().cloned().map(Group::Template));
        groups.extend(report.environments.iter().cloned().map(Group::Environment));
    }
    for problem in &report.problems {
        let group = Group::of(problem);
        if !groups.contains(&group) {
            groups.push(group);
        }
    }
    for group in &groups {
        let heading = style::title_with_color(group.heading(paths), color);
        let problems: Vec<&Problem> = report
            .problems
            .iter()
            .filter(|problem| Group::of(problem) == *group)
            .collect();
        if problems.is_empty() {
            writeln!(out, "{heading}  ok")?;
            continue;
        }
        writeln!(out, "{heading}")?;
        for problem in problems {
            let level = format!("{:<7}", problem.level.as_str());
            let level = match problem.level {
                Level::Error => style::error_with_color(level, color),
                Level::Warning => style::warning_with_color(level, color),
            };
            let mut lines = problem.message.lines();
            writeln!(out, "  {level}  {}", lines.next().unwrap_or_default())?;
            for line in lines {
                writeln!(out, "           {line}")?;
            }
        }
    }
    Ok(())
}

/// Like `1 error, 2 warnings`, leaving out a level with none.
fn counts(report: &Report) -> String {
    let mut parts = Vec::new();
    if report.errors() > 0 {
        parts.push(text::counted(report.errors(), "error", "errors"));
    }
    if report.warnings() > 0 {
        parts.push(text::counted(report.warnings(), "warning", "warnings"));
    }
    parts.join(", ")
}

fn summary(report: &Report) -> String {
    format!("{}.", counts(report))
}

fn clean(report: &Report, name: Option<&str>) -> String {
    match name {
        Some(name) => format!("No problems in {name}."),
        None => format!(
            "No problems in {}.",
            text::counted(report.environments.len(), "environment", "environments")
        ),
    }
}

fn fixed_line(fixed: usize, after: &Report) -> String {
    let done = if fixed == 0 {
        "Nothing was fixed.".to_owned()
    } else {
        format!("Fixed {}.", text::counted(fixed, "problem", "problems"))
    };
    if after.problems.is_empty() {
        done
    } else {
        format!("{done} {} left.", counts(after))
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use wineo_core::components::Component;
    use wineo_core::validate::{Area, Code};

    use super::*;
    use crate::prompt::Script;

    fn paths(home: &Path) -> Paths {
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(home))).unwrap()
    }

    fn problem(code: Code, environment: Option<&str>, message: &str, fix: Option<Fix>) -> Problem {
        Problem {
            environment: environment.map(str::to_owned),
            area: code.area(),
            level: code.level(),
            code,
            message: message.to_owned(),
            fix,
            stray: None,
            template: None,
        }
    }

    fn art_fix() -> Fix {
        Fix::DropArtRecord {
            environment: "zomboid".to_owned(),
            target: None,
            kind: wineo_core::art::Kind::Icon,
        }
    }

    fn state_fix() -> Fix {
        Fix::RemoveStateDir {
            environment: "oldgame".to_owned(),
        }
    }

    fn broken() -> Report {
        let mut stray = problem(
            Code::StateDirStray,
            None,
            "state dir /s/oldgame has no environment",
            Some(state_fix()),
        );
        stray.stray = Some("oldgame".to_owned());
        let mut template = problem(Code::TemplateInvalid, None, "invalid config t.toml", None);
        template.template = Some("mine".to_owned());
        Report {
            environments: vec!["clean".to_owned(), "zomboid".to_owned()],
            templates: vec!["mine".to_owned()],
            problems: vec![
                problem(
                    Code::GlobalConfigInvalid,
                    None,
                    "invalid config\nline two",
                    None,
                ),
                template,
                problem(
                    Code::ExeMissing,
                    Some("zomboid"),
                    "target zomboid: C:\\Z.exe not found",
                    None,
                ),
                problem(
                    Code::ArtFileMissing,
                    Some("zomboid"),
                    "art: icon.png is recorded but missing",
                    Some(art_fix()),
                ),
                stray,
            ],
        }
    }

    fn printed(report: &Report, verbose: bool) -> String {
        printed_as(report, verbose, false)
    }

    fn printed_as(report: &Report, verbose: bool, named: bool) -> String {
        let root = tempfile::tempdir().unwrap();
        let mut out = Vec::new();
        print_report(&paths(root.path()), report, verbose, named, false, &mut out).unwrap();
        String::from_utf8(out)
            .unwrap()
            .replace(&root.path().display().to_string(), "~")
    }

    #[test]
    fn problems_are_grouped_under_what_they_are_in() {
        assert_eq!(
            printed(&broken(), false),
            concat!(
                "~/.config/wineo/config.toml\n",
                "  error    invalid config\n",
                "           line two\n",
                "~/.config/wineo/templates/mine.toml\n",
                "  error    invalid config t.toml\n",
                "zomboid\n",
                "  error    target zomboid: C:\\Z.exe not found\n",
                "  warning  art: icon.png is recorded but missing\n",
                "oldgame\n",
                "  warning  state dir /s/oldgame has no environment\n",
            )
        );
    }

    #[test]
    fn verbose_lists_everything_checked() {
        let listed = printed(&broken(), true);
        assert!(listed.contains("~/.config/wineo/templates/mine.toml\n  error"));
        assert!(listed.contains("\nclean  ok\nzomboid\n"), "{listed}");

        let report = Report {
            environments: vec!["zomboid".to_owned()],
            ..Report::default()
        };
        assert_eq!(printed_as(&report, true, true), "zomboid  ok\n");
        assert_eq!(
            printed(&report, true),
            "~/.config/wineo/config.toml  ok\nzomboid  ok\n"
        );
    }

    #[test]
    fn counts_and_endings_read_right() {
        let report = broken();
        assert_eq!(summary(&report), "3 errors, 2 warnings.");
        let one = Report {
            problems: vec![broken().problems.remove(4)],
            ..Report::default()
        };
        assert_eq!(summary(&one), "1 warning.");
        assert_eq!(fixed_line(2, &Report::default()), "Fixed 2 problems.");
        assert_eq!(
            fixed_line(1, &report),
            "Fixed 1 problem. 3 errors, 2 warnings left."
        );
        assert_eq!(fixed_line(0, &one), "Nothing was fixed. 1 warning left.");
        let none = Report {
            environments: vec!["a".to_owned()],
            ..Report::default()
        };
        assert_eq!(clean(&none, None), "No problems in 1 environment.");
        assert_eq!(
            clean(&Report::default(), None),
            "No problems in 0 environments."
        );
        assert_eq!(clean(&none, Some("a")), "No problems in a.");
    }

    #[test]
    fn equal_downloads_are_offered_once() {
        let download = Fix::Download {
            component: Component::Dxvk,
            version: None,
        };
        let report = Report {
            problems: vec![
                problem(Code::BuildMissing, Some("a"), "x", Some(download.clone())),
                problem(Code::BuildMissing, Some("b"), "x", Some(download.clone())),
                problem(Code::LostRun, Some("b"), "x", Some(state_fix())),
            ],
            ..Report::default()
        };
        assert_eq!(report.fixes(), [download, state_fix()]);
        assert_eq!(report.problems[0].area, Area::Wine);
    }

    #[test]
    fn enter_fixes_everything_listed() {
        let mut prompt = Prompt::new(Script::new(&[""]), Vec::new());
        let chosen = review(&mut prompt, "1 warning.", vec![art_fix(), state_fix()]).unwrap();
        assert_eq!(chosen, [art_fix(), state_fix()]);
        let printed = String::from_utf8(prompt.output().clone()).unwrap();
        assert_eq!(
            printed,
            concat!(
                "1 warning.\n",
                "  1) Drop zomboid's missing icon record\n",
                "  2) Remove oldgame's leftover state\n",
                "Numbers skip those fixes, like 1 3 4.\n",
                "\n",
                "Press Enter to fix them, or pick numbers. Ctrl+C to cancel.\n",
                "Choice: ",
            )
        );
    }

    #[test]
    fn numbers_skip_those_fixes() {
        let mut prompt = Prompt::new(Script::new(&["1", ""]), Vec::new());
        let chosen = review(&mut prompt, "h", vec![art_fix(), state_fix()]).unwrap();
        assert_eq!(chosen, [state_fix()]);

        let lost = Fix::CleanUpLostRun {
            environment: "a".to_owned(),
        };
        let mut prompt = Prompt::new(Script::new(&["1, 3", ""]), Vec::new());
        let chosen = review(&mut prompt, "h", vec![art_fix(), state_fix(), lost.clone()]).unwrap();
        assert_eq!(chosen, [state_fix()]);

        let mut prompt = Prompt::new(Script::new(&["1 2"]), Vec::new());
        assert_eq!(review(&mut prompt, "h", vec![art_fix(), lost]).unwrap(), []);

        let mut prompt = Prompt::new(Script::new(&["1"]), Vec::new());
        assert_eq!(review(&mut prompt, "h", vec![art_fix()]).unwrap(), []);
    }

    #[test]
    fn cancel_fixes_nothing() {
        let mut prompt = Prompt::new(Script::new(&["q"]), Vec::new());
        let err = review(&mut prompt, "h", vec![art_fix()]).unwrap_err();
        assert!(cancel::prompted(&err));
    }
}
