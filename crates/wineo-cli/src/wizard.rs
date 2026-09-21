//! Guided `wineo new`. Every question comes before anything is written, and
//! nothing is created until the review is accepted. Creating then runs the
//! installer and carries on into the same target picker `setup` uses.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use wineo_core::config::{Arch, GlobalConfig};
use wineo_core::environment::{Environment, NewEnvironment};
use wineo_core::install::{self, Wanted};
use wineo_core::installer::Installer;
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::prefix;
use wineo_core::programs::{Program, Source};
use wineo_core::runner::Runner;
use wineo_core::runners::Runners;
use wineo_core::targets::Batch;
use wineo_core::templates::{NEWEST_PROTON, Template, Templates};
use wineo_core::winpath;

use crate::commands::setup::{self, Ran};
use crate::commands::{components, desktop};
use crate::picker::{self, Place};
use crate::prompt::{Canceled, Input, Prompt, Reply, numbered};

/// What the command line already said, which the questions start from.
#[derive(Debug, Default, Clone)]
pub struct Prefill {
    pub name: Option<String>,
    pub dir: Option<String>,
    pub runner: Option<String>,
    pub template: Option<String>,
    pub arch: Option<Arch>,
    pub install_exe: Option<String>,
}

/// How the new environment gets its program.
#[derive(Debug, Clone, PartialEq, Eq)]
enum How {
    Installer(String),
    /// Already on disk, as a target's `exe` would hold it.
    Existing(String),
    Nothing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Template,
    Name,
    Location,
    Settings,
    How,
    Review,
}

/// The answers so far. Each question starts from what is here.
struct Answers {
    template: Option<(String, Template)>,
    name: Option<String>,
    dir: Option<String>,
    runner: Option<String>,
    arch: Option<Arch>,
    /// Whether `arch` came from a flag or the template, which an existing
    /// prefix may not quietly override.
    arch_asked: bool,
    how: Option<How>,
}

/// What guided creation came to, for the exit code and the last words.
pub struct Guided {
    /// The environment once it is registered, so a cancel after that point
    /// says it was kept.
    pub registered: Option<String>,
}

pub fn run<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    prefill: &Prefill,
    prompt: &mut Prompt<I, W>,
    guided: &mut Guided,
) -> anyhow::Result<ExitCode> {
    let templates = Templates::new(paths);
    let template = match &prefill.template {
        Some(name) => Some((name.clone(), templates.resolve(name)?)),
        None => None,
    };
    let mut answers = Answers {
        arch_asked: prefill.arch.is_some()
            || template.as_ref().is_some_and(|(_, t)| t.arch.is_some()),
        arch: prefill.arch,
        template,
        name: prefill.name.clone(),
        dir: prefill.dir.clone(),
        runner: prefill.runner.clone(),
        how: prefill.install_exe.clone().map(How::Installer),
    };
    // A template from the command line isn't asked about, but going back
    // still reaches the question.
    let mut step = if prefill.template.is_some() {
        Step::Name
    } else {
        Step::Template
    };
    let (runner, arch, dir) = loop {
        match step {
            Step::Template => {
                let current = answers.template.as_ref().map(|(name, _)| name.as_str());
                let Reply::Value(template) = ask_template(prompt, &templates, current)? else {
                    unreachable!("the first question has nowhere to go back to");
                };
                answers.arch_asked = prefill.arch.is_some()
                    || template.as_ref().is_some_and(|(_, t)| t.arch.is_some());
                answers.template = template;
                step = Step::Name;
            }
            Step::Name => match ask_name(prompt, paths, answers.name.as_deref())? {
                Reply::Value(Name::New(name)) => {
                    answers.name = Some(name);
                    step = Step::Location;
                }
                Reply::Value(Name::Existing(name)) => {
                    guided.registered = Some(name.clone());
                    return setup::setup(paths, global, &name, prompt);
                }
                Reply::Back => step = Step::Template,
            },
            Step::Location => {
                let name = answers.name.clone().expect("named before this");
                match ask_location(prompt, paths, global, &name, answers.dir.as_deref())? {
                    Reply::Value(dir) => {
                        answers.dir = Some(dir);
                        step = Step::Settings;
                    }
                    Reply::Back => step = Step::Name,
                }
            }
            Step::Settings => match settings(prompt, paths, global, &mut answers)? {
                Reply::Value(()) => step = Step::How,
                // Always back to the template, which decides the rest.
                Reply::Back => step = Step::Template,
            },
            Step::How => match ask_how(prompt, paths, &answers)? {
                Reply::Value(how) => {
                    answers.how = Some(how);
                    step = Step::Review;
                }
                Reply::Back => step = Step::Settings,
            },
            Step::Review => match review(prompt, paths, global, &answers)? {
                Reply::Value(chosen) => break chosen,
                Reply::Back => step = Step::How,
            },
        }
    };

    let name = answers.name.clone().expect("named before the review");
    let how = answers.how.clone().expect("chosen before the review");
    let template = answers
        .template
        .map(|(_, template)| template.for_runner(&runner).0);
    create(
        paths,
        global,
        prompt,
        guided,
        Creating {
            name,
            dir,
            runner,
            arch,
            template,
            how,
        },
    )
}

enum Name {
    New(String),
    /// Already registered, and to be set up instead.
    Existing(String),
}

fn ask_template<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    templates: &Templates,
    current: Option<&str>,
) -> anyhow::Result<Reply<Option<(String, Template)>>> {
    let mut entries = vec![("None, choose each setting".to_owned(), None)];
    let mut broken = Vec::new();
    for listed in templates.list()? {
        match templates.resolve(&listed.name) {
            Ok(template) => entries.push((
                format!("{}  ({})", listed.name, describe_template(&template)),
                Some((listed.name.clone(), template)),
            )),
            Err(err) => broken.push(format!("{}: {err}", listed.name)),
        }
    }
    let default = current
        .and_then(|current| {
            entries
                .iter()
                .position(|(_, entry)| entry.as_ref().is_some_and(|(name, _)| name == current))
        })
        .map_or(1, |index| index + 1)
        .to_string();
    let mut heading = "Start from a template?".to_owned();
    for note in broken {
        heading.push_str(&format!("\nCan't use {note}"));
    }
    prompt.choose(
        &heading,
        "Template",
        &numbered(entries),
        Some(&default),
        false,
    )
}

/// A template line for a card, with what was left out for the runner.
fn shown_template(template: Option<&(String, Template)>, runner: Option<&Runner>) -> String {
    let Some((name, template)) = template else {
        return "none".to_owned();
    };
    // The runner has its own line on a card.
    let template = &Template {
        runner: None,
        ..template.clone()
    };
    match runner.map(|runner| template.for_runner(runner)) {
        Some((fitted, true)) => format!(
            "{name} ({}; no DXVK or vkd3d-proton, Proton has its own)",
            describe_template(&fitted)
        ),
        _ => format!("{name} ({})", describe_template(template)),
    }
}

/// What a template sets, as a short list.
fn describe_template(template: &Template) -> String {
    let mut parts = Vec::new();
    match template.runner.as_deref() {
        Some(NEWEST_PROTON) => parts.push("Proton when installed".to_owned()),
        Some(runner) => parts.push(format!("runner {runner}")),
        None => {}
    }
    if let Some(arch) = template.arch {
        parts.push(arch.to_string());
    }
    if let Some(dxvk) = &template.dxvk {
        parts.push(format!("DXVK {dxvk}"));
    }
    if let Some(vkd3d) = &template.vkd3d {
        parts.push(format!("vkd3d-proton {vkd3d}"));
    }
    match template.gamemode {
        Some(true) => parts.push("gamemode on".to_owned()),
        Some(false) => parts.push("gamemode off".to_owned()),
        None => {}
    }
    if let Some(gameid) = &template.gameid {
        parts.push(format!("gameid {gameid}"));
    }
    if let Some(store) = &template.store {
        parts.push(format!("store {store}"));
    }
    match template.env.len() {
        0 => {}
        1 => parts.push("1 environment variable".to_owned()),
        n => parts.push(format!("{n} environment variables")),
    }
    if parts.is_empty() {
        "sets nothing".to_owned()
    } else {
        parts.join(", ")
    }
}

fn ask_name<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    paths: &Paths,
    default: Option<&str>,
) -> anyhow::Result<Reply<Name>> {
    prompt.say("Used in commands like `wineo start <name>` and as the prefix folder name.")?;
    loop {
        let reply = prompt.text(
            "Install name",
            default,
            |answer| match picker::name_problem(answer) {
                Some(problem) => Err(problem),
                None => Ok(answer.to_owned()),
            },
        )?;
        let Reply::Value(name) = reply else {
            return Ok(Reply::Back);
        };
        if !paths.environment_config(&name).exists() {
            return Ok(Reply::Value(Name::New(name)));
        }
        let heading = format!("{name} already exists.");
        let choices = numbered([
            (format!("Set up {name} instead"), true),
            ("Pick another name".to_owned(), false),
        ]);
        if prompt.choose(&heading, "Choice", &choices, Some("1"), true)? == Reply::Value(true) {
            return Ok(Reply::Value(Name::Existing(name)));
        }
    }
}

fn ask_location<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    default: Option<&str>,
) -> anyhow::Result<Reply<String>> {
    let suggested = match default {
        Some(default) => default.to_owned(),
        None => {
            let dir = global.prefix_root(paths)?.join(name);
            paths
                .contract(&dir)
                .unwrap_or_else(|_| dir.display().to_string())
        }
    };
    let reply = prompt.text("Prefix location", Some(&suggested), |answer| {
        let answer = &picker::clean_path(answer);
        let dir = paths.absolute(answer).map_err(|err| err.to_string())?;
        match fs::read_dir(&dir).map(|mut entries| entries.next().is_some()) {
            Ok(true) if !prefix::holds_prefix(&dir) => {
                Err("Not empty, and not a wine prefix.".to_owned())
            }
            Ok(_) => Ok(answer.to_owned()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(answer.to_owned()),
            Err(err) => Err(format!("{answer} can't be used: {err}")),
        }
    })?;
    if let Reply::Value(dir) = &reply
        && let Ok(absolute) = paths.absolute(dir)
        && prefix::holds_prefix(&absolute)
    {
        prompt.say("Using the existing prefix there.")?;
    }
    Ok(reply)
}

/// The runner and architecture, shown with where each came from, accepted
/// as they are or changed. Anything that can't work together is explained
/// instead of offered.
fn settings<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    paths: &Paths,
    global: &GlobalConfig,
    answers: &mut Answers,
) -> anyhow::Result<Reply<()>> {
    let runners = Runners::new(paths, global)?;
    let name = answers.name.clone().expect("named before this");
    let dir = paths.absolute(answers.dir.as_deref().expect("placed before this"))?;
    loop {
        let template = answers.template.as_ref().map(|(_, template)| template);
        let from_template = match template {
            Some(template) => template.runner_name(&runners, &global.defaults.runner)?,
            None => None,
        };
        let runner_name = answers
            .runner
            .clone()
            .or(from_template)
            .unwrap_or_else(|| global.defaults.runner.clone());
        let arch = answers
            .arch
            .or_else(|| template.and_then(|template| template.arch))
            .unwrap_or(global.defaults.arch);
        let runner = runners.resolve(&runner_name);
        let problem = match &runner {
            Err(err) => Some(format!("That runner can't be used: {err}")),
            Ok(runner) => combination_problem(paths, global, &name, &dir, runner, arch, answers),
        };

        let mut heading = format!("Settings for {name}:\n");
        heading.push_str(&format!(
            "  runner    {}\n",
            match &runner {
                Ok(runner) => describe_runner(runner),
                Err(_) => format!("{runner_name}, not usable"),
            }
        ));
        let adopted = runner
            .as_ref()
            .ok()
            .and_then(|runner| adopted_arch(runner, &dir));
        let shown_arch = match (&runner, adopted) {
            (Ok(runner), _) if runner.is_proton() => "win64, as every Proton prefix is".to_owned(),
            (_, Some(adopted)) => format!("{adopted}, as the existing prefix is"),
            _ => arch.to_string(),
        };
        heading.push_str(&format!("  arch      {shown_arch}\n"));
        heading.push_str(&format!(
            "  template  {}",
            shown_template(answers.template.as_ref(), runner.as_ref().ok())
        ));
        if let Some(problem) = &problem {
            heading.push('\n');
            heading.push_str(problem);
        }

        #[derive(Clone, Copy)]
        enum Change {
            Accept,
            Runner,
            Download,
            Arch,
        }
        let mut entries = Vec::new();
        if problem.is_none() {
            entries.push(("Use these".to_owned(), Change::Accept));
        }
        entries.push(("Change the runner".to_owned(), Change::Runner));
        entries.push((
            "Download a runner or graphics tool".to_owned(),
            Change::Download,
        ));
        if adopted.is_none() {
            entries.push(("Change the arch".to_owned(), Change::Arch));
        }
        entries.push((
            "Download a runner or graphics tool".to_owned(),
            Change::Download,
        ));
        match prompt.choose(&heading, "Choice", &numbered(entries), Some("1"), true)? {
            Reply::Back => return Ok(Reply::Back),
            Reply::Value(Change::Accept) => {
                answers.runner = Some(runner_name);
                answers.arch = Some(arch);
                return Ok(Reply::Value(()));
            }
            Reply::Value(Change::Runner) => {
                if let Reply::Value(chosen) = pick_runner(prompt, &runners, &runner_name)? {
                    answers.runner = Some(chosen);
                }
            }
            Reply::Value(Change::Download) => {
                let _ = crate::commands::download::choose_and_install(
                    paths,
                    global,
                    prompt,
                    &crate::commands::download::PROVIDERS,
                )?;
            }
            Reply::Value(Change::Arch) => {
                let choices = numbered([
                    ("win64".to_owned(), Arch::Win64),
                    ("win32".to_owned(), Arch::Win32),
                ]);
                let default = if arch == Arch::Win32 { "2" } else { "1" };
                if let Reply::Value(chosen) =
                    prompt.choose("Which architecture?", "Arch", &choices, Some(default), true)?
                {
                    answers.arch = Some(chosen);
                    answers.arch_asked = true;
                }
            }
        }
    }
}

/// Why this runner can't be used with the rest of the answers, if it can't.
fn combination_problem(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    dir: &Path,
    runner: &Runner,
    arch: Arch,
    answers: &Answers,
) -> Option<String> {
    if runner.is_proton() && arch == Arch::Win32 && answers.arch_asked {
        return Some("Proton prefixes are always win64.".to_owned());
    }
    // The layout decides which kind of runner an existing prefix is for.
    let wine_prefix = runner.wine_prefix(dir);
    let occupied = fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some());
    if occupied && !prefix::is_prefix(&wine_prefix) {
        return Some(format!(
            "{} holds a {} prefix, which {} can't use.",
            dir.display(),
            if runner.is_proton() { "wine" } else { "Proton" },
            runner.name()
        ));
    }
    if answers.arch_asked
        && let Some(adopted) = adopted_arch(runner, dir)
        && adopted != arch
        && !runner.is_proton()
    {
        return Some(format!("The existing prefix is {adopted}."));
    }
    if let Some((template_name, template)) = &answers.template {
        let (template, _) = template.for_runner(runner);
        let wanted = Wanted {
            dxvk: template.dxvk.clone(),
            vkd3d: template.vkd3d.clone(),
        };
        if let Err(err) = install::check_available(paths, global, name, runner, &wanted) {
            return Some(format!("Can't use the {template_name} template: {err}."));
        }
    }
    None
}

fn adopted_arch(runner: &Runner, dir: &Path) -> Option<Arch> {
    let wine_prefix = runner.wine_prefix(dir);
    if !prefix::is_prefix(&wine_prefix) {
        return None;
    }
    prefix::detect_arch(&wine_prefix).ok().flatten()
}

fn describe_runner(runner: &Runner) -> String {
    let mut text = if runner.selection() != runner.name() {
        format!("{} -> {}", runner.selection(), runner.name())
    } else {
        runner.name().to_owned()
    };
    let version = runner.version();
    match (runner.is_proton(), version) {
        (true, Some(version)) if version != runner.name() => {
            text.push_str(&format!(" (Proton, {version})"));
        }
        (true, _) => text.push_str(" (Proton)"),
        (false, Some(version)) => text.push_str(&format!(" ({version})")),
        (false, None) => {}
    }
    text
}

fn pick_runner<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    runners: &Runners,
    current: &str,
) -> anyhow::Result<Reply<String>> {
    let listing = runners.list()?;
    let mut entries = Vec::new();
    let mut unusable = Vec::new();
    match Runner::system() {
        Ok(runner) => entries.push((
            format!("{}, the system wine", describe_runner(&runner)),
            Runner::SYSTEM.to_owned(),
        )),
        Err(err) => unusable.push(format!("{}: {err}", Runner::SYSTEM)),
    }
    for runner in &listing.runners {
        entries.push((describe_runner(runner), runner.name().to_owned()));
    }
    for alias in wineo_core::runner_alias::ALIASES {
        match runners.resolve(alias) {
            Ok(runner) => entries.push((describe_runner(&runner), alias.to_owned())),
            Err(err) => unusable.push(err.to_string()),
        }
    }
    for (name, reason) in &listing.skipped {
        unusable.push(format!("{name}: {}", reason.as_str()));
    }
    let mut heading = "Which runner?".to_owned();
    if !unusable.is_empty() {
        heading.push_str(&format!("\nNot usable, in {}:", runners.root().display()));
        for note in unusable {
            heading.push_str(&format!("\n  {note}"));
        }
    }
    let default = entries
        .iter()
        .position(|(_, name)| name == current)
        .map(|index| (index + 1).to_string());
    prompt.choose(
        &heading,
        "Runner",
        &numbered(entries),
        default.as_deref(),
        true,
    )
}

fn ask_how<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    paths: &Paths,
    answers: &Answers,
) -> anyhow::Result<Reply<How>> {
    let name = answers.name.as_deref().expect("named before this");
    let dir = paths.absolute(answers.dir.as_deref().expect("placed before this"))?;
    // A Windows path can only name something in a prefix that already exists.
    let wine_prefix = prefix::wine_prefix_in(&dir);
    loop {
        #[derive(Clone, Copy, PartialEq)]
        enum Way {
            Installer,
            Existing,
            Nothing,
        }
        let choices = numbered([
            (
                "Run an installer, a local .exe or .msi".to_owned(),
                Way::Installer,
            ),
            (
                "Use a program that is already there, installed or extracted".to_owned(),
                Way::Existing,
            ),
            ("Nothing for now".to_owned(), Way::Nothing),
        ]);
        let default = match &answers.how {
            Some(How::Existing(_)) => "2",
            Some(How::Nothing) => "3",
            _ => "1",
        };
        let heading = format!("How does {name} get its program?");
        let way = match prompt.choose(&heading, "Choice", &choices, Some(default), true)? {
            Reply::Value(way) => way,
            Reply::Back => return Ok(Reply::Back),
        };
        let reply = match way {
            Way::Nothing => return Ok(Reply::Value(How::Nothing)),
            Way::Installer => {
                let current = match &answers.how {
                    Some(How::Installer(path)) => Some(path.as_str()),
                    _ => None,
                };
                prompt
                    .text("Installer", current, |answer| {
                        let answer = picker::clean_path(answer);
                        local_file(paths, wine_prefix.as_deref(), &answer).map(|_| answer)
                    })?
                    .map(How::Installer)
            }
            Way::Existing => {
                let current = match &answers.how {
                    Some(How::Existing(exe)) => Some(exe.as_str()),
                    _ => None,
                };
                prompt
                    .text("Executable", current, |answer| {
                        let answer = picker::clean_path(answer);
                        let unix = local_file(paths, wine_prefix.as_deref(), &answer)?;
                        Ok(if winpath::is_windows_path(&answer) {
                            answer
                        } else {
                            wine_prefix
                                .as_deref()
                                .and_then(|wine_prefix| winpath::from_unix(wine_prefix, &unix))
                                .or_else(|| paths.contract(&unix).ok())
                                .unwrap_or_else(|| unix.display().to_string())
                        })
                    })?
                    .map(How::Existing)
            }
        };
        if let Reply::Value(how) = reply {
            return Ok(Reply::Value(how));
        }
    }
}

/// A file that has to exist before anything is created: a Unix path, or a
/// Windows path inside a prefix that is already there.
fn local_file(paths: &Paths, wine_prefix: Option<&Path>, answer: &str) -> Result<PathBuf, String> {
    if answer.is_empty() {
        return Err("Enter a path".to_owned());
    }
    let found = if winpath::is_windows_path(answer) {
        wine_prefix.and_then(|wine_prefix| winpath::to_unix(wine_prefix, answer))
    } else {
        paths.absolute(answer).ok()
    };
    match found {
        Some(path) if path.is_file() => Ok(path),
        _ if winpath::is_windows_path(answer) && wine_prefix.is_none() => {
            Err("Use a Unix path, the prefix doesn't exist yet.".to_owned())
        }
        _ => Err(format!("`{answer}` does not exist")),
    }
}

/// Everything creation needs, checked again right before it happens.
struct Creating {
    name: String,
    dir: PathBuf,
    runner: Runner,
    arch: Arch,
    template: Option<Template>,
    how: How,
}

fn review<I: Input, W: Write>(
    prompt: &mut Prompt<I, W>,
    paths: &Paths,
    global: &GlobalConfig,
    answers: &Answers,
) -> anyhow::Result<Reply<(Runner, Arch, PathBuf)>> {
    let name = answers.name.as_deref().expect("named before the review");
    let shown_dir = answers.dir.as_deref().expect("placed before the review");
    let dir = paths.absolute(shown_dir)?;
    let runner = Runners::new(paths, global)?
        .resolve(answers.runner.as_deref().expect("chosen in the settings"))?;
    let adopted = adopted_arch(&runner, &dir);
    let arch = if runner.is_proton() {
        Arch::Win64
    } else {
        adopted.unwrap_or(answers.arch.unwrap_or(global.defaults.arch))
    };
    let existing = prefix::is_prefix(&runner.wine_prefix(&dir));

    let mut heading = format!("Ready to create {name}:\n");
    heading.push_str(&format!(
        "  location  {shown_dir}{}\n",
        if existing {
            ", using the prefix already there"
        } else {
            ", made new"
        }
    ));
    heading.push_str(&format!("  runner    {}\n", describe_runner(&runner)));
    heading.push_str(&format!("  arch      {arch}\n"));
    heading.push_str(&format!(
        "  template  {}\n",
        shown_template(answers.template.as_ref(), Some(&runner))
    ));
    heading.push_str(&format!(
        "  program   {}",
        match answers.how.as_ref().expect("chosen before the review") {
            How::Installer(path) => format!("run the installer {path}"),
            How::Existing(exe) => format!("use {exe}"),
            How::Nothing => "none for now".to_owned(),
        }
    ));
    if runner.is_proton() {
        heading.push_str(
            "\nThe first umu run downloads the Steam Linux Runtime, which takes a while.",
        );
    }
    let choices = numbered([("Create it".to_owned(), ())]);
    Ok(
        match prompt.choose(&heading, "Choice", &choices, Some("1"), true)? {
            Reply::Value(()) => Reply::Value((runner, arch, dir)),
            Reply::Back => Reply::Back,
        },
    )
}

fn create<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    prompt: &mut Prompt<I, W>,
    guided: &mut Guided,
    creating: Creating,
) -> anyhow::Result<ExitCode> {
    let Creating {
        name,
        dir,
        runner,
        arch,
        template,
        how,
    } = creating;
    let new = NewEnvironment {
        name: name.clone(),
        dir: Some(dir.display().to_string()),
        arch: (!runner.is_proton()).then_some(arch),
        template,
    };
    let guard = Locks::new(paths).acquire(Operation::Create, &name, &[&dir])?;
    let shown = paths
        .contract(&dir)
        .unwrap_or_else(|_| dir.display().to_string());
    let mut created = false;
    let environment = match Environment::create(paths, global, &runner, &new, |_| {
        created = true;
        let _ = prompt.say(if runner.is_proton() {
            format!("Creating a Proton prefix in {shown}, which takes a while")
        } else {
            format!("Creating a wine prefix in {shown}, which takes a few seconds")
        });
    }) {
        Ok(environment) => environment,
        Err(err) => {
            if prompt.input().interrupted() {
                return Err(Canceled.into());
            }
            prompt.say(format!("error: {:#}", anyhow::Error::from(err)))?;
            prompt.say(format!(
                "{name} wasn't created. Fix that, then run `wineo new` again."
            ))?;
            return Ok(ExitCode::FAILURE);
        }
    };
    guided.registered = Some(name.clone());
    prompt.say(if created {
        format!("Created {name}.")
    } else {
        format!("Created {name}, using the prefix already at {shown}.")
    })?;
    components::reconcile_config(paths, global, &environment, &runner, &dir)?;
    desktop::reconcile(paths, global, &environment, Some(&runner), &dir)?;

    let place = Place {
        paths,
        name: &name,
        wine_prefix: Some(runner.wine_prefix(&dir)),
    };
    let mut batch = Batch::new(environment.config.targets.clone());
    match how {
        How::Nothing => {
            drop(guard);
            return setup::finish(paths, global, &name, prompt, &batch);
        }
        How::Installer(program) => {
            let installer =
                Installer::prepare_with_runner(guard, paths, global, &name, &program, &runner)?;
            match setup::run_installer(paths, global, &name, prompt, installer)? {
                Ran::Finished => picker::pick(prompt, &place, &mut batch).map(|_| ())?,
                Ran::Ended(code) => return Ok(code),
                Ran::Back => unreachable!("an installer that ran never goes back"),
            }
        }
        How::Existing(exe) => {
            drop(guard);
            let program = Program {
                source: Source::Executable,
                name: file_stem(&exe),
                exe,
                lnk: None,
            };
            picker::add_program(prompt, &place, &mut batch, &program)?;
        }
    }
    if batch.is_empty() {
        return setup::finish(paths, global, &name, prompt, &batch);
    }
    match setup::save(paths, global, &name, prompt, &mut batch)? {
        Reply::Value(()) => setup::finish(paths, global, &name, prompt, &batch),
        // Keeping on editing is what `setup` is for.
        Reply::Back => setup::menu(paths, global, &name, prompt, &place, batch),
    }
}

fn file_stem(path: &str) -> String {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    file.rsplit_once('.')
        .map_or(file, |(stem, _)| stem)
        .to_owned()
}
