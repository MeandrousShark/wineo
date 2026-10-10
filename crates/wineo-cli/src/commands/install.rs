use std::io::IsTerminal;
use std::process::ExitCode;

use wineo_core::config::GlobalConfig;
use wineo_core::paths::Paths;
use wineo_core::steam_install::{self, Action, End, Follow, Outcome, Update};

use super::transfer::Cancellation;
use crate::style;

#[derive(clap::Args)]
pub struct Args {
    /// Steam game to install
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: String,

    /// Stay attached and show progress until Steam is done
    #[arg(short, long)]
    follow: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    act(paths, &args.name, args.follow, Action::Install)
}

/// Opens Steam's dialog for `action`, then follows it when asked. Shared by
/// `install` and `uninstall`.
pub fn act(paths: &Paths, name: &str, follow: bool, action: Action) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let outcome = steam_install::request(paths, &global, name, action)?;
    match outcome.note() {
        Some(note) => println!("{note}"),
        None if !follow => match action {
            Action::Install => println!("Asked Steam to install {name}."),
            Action::Uninstall => println!("Asked Steam to uninstall {name}."),
        },
        None => {}
    }
    // An install Steam is already running can be followed too.
    if follow && matches!(outcome, Outcome::Opened | Outcome::AlreadyInstalling(_)) {
        self::follow(paths, &global, name, action)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn follow(
    paths: &Paths,
    global: &GlobalConfig,
    name: &str,
    action: Action,
) -> anyhow::Result<ExitCode> {
    let mut follow = Follow::new(paths, global, name, action)?;
    let title = follow.title().to_owned();
    // Ctrl+C stops following. Steam's work is its own and carries on.
    let cancellation = Cancellation::new()?;
    let mut screen = Screen::new(std::io::stdout().is_terminal());
    let end = follow.wait(&cancellation.token, |update| {
        if let Some(line) = line(action, &title, update) {
            screen.show(update, line);
        }
    });
    screen.finish();
    match (end, action) {
        (End::Finished, Action::Install) => println!("Installed."),
        (End::Finished, Action::Uninstall) => println!("Uninstalled."),
        (End::SteamCanceled, Action::Install) => {
            eprintln!("Steam canceled the install.");
            return Ok(ExitCode::FAILURE);
        }
        (End::SteamCanceled, Action::Uninstall) => {
            eprintln!("Steam canceled the uninstall.");
            return Ok(ExitCode::FAILURE);
        }
        (End::Stopped, _) => return Ok(ExitCode::from(130)),
    }
    Ok(ExitCode::SUCCESS)
}

/// The progress line for one look at the manifest. The last looks, which end
/// the follow, are said by the caller.
fn line(action: Action, title: &str, update: &Update) -> Option<String> {
    match update {
        Update::Waiting => Some(match action {
            Action::Install => "Waiting for Steam's install dialog...".to_owned(),
            Action::Uninstall => format!("Uninstalling {title}..."),
        }),
        Update::Queued => Some(format!("Install queued for {title}...")),
        Update::Installing {
            percent,
            done,
            total,
        } => Some(match percent {
            Some(percent) if *total > 0 => format!(
                "Installing {title}... {percent}% ({} of {})",
                style::size(*done),
                style::size(*total)
            ),
            Some(percent) => format!("Installing {title}... {percent}%"),
            None => format!("Installing {title}..."),
        }),
        Update::Uninstalling => Some(format!("Uninstalling {title}...")),
        Update::Unreadable(reason) => Some(format!("Can't read Steam's files: {reason}")),
        Update::Finished | Update::SteamCanceled => None,
    }
}

/// Draws progress lines: redrawn in place on a terminal, and one line per
/// change otherwise, so a log stays readable.
struct Screen {
    terminal: bool,
    /// What the last printed line was about, to skip repeats off a terminal.
    shown: Option<Key>,
    open: bool,
}

#[derive(PartialEq)]
struct Key {
    kind: std::mem::Discriminant<Update>,
    percent: Option<u8>,
    reason: Option<String>,
}

impl Screen {
    fn new(terminal: bool) -> Self {
        Self {
            terminal,
            shown: None,
            open: false,
        }
    }

    fn show(&mut self, update: &Update, line: String) {
        let key = Key {
            kind: std::mem::discriminant(update),
            percent: match update {
                Update::Installing { percent, .. } => *percent,
                _ => None,
            },
            reason: match update {
                Update::Unreadable(reason) => Some(reason.clone()),
                _ => None,
            },
        };
        if self.terminal {
            // Erases what a longer earlier line left behind.
            print!("\r\x1b[K{line}");
            self.open = true;
        } else if self.shown.as_ref() != Some(&key) {
            println!("{line}");
        }
        self.shown = Some(key);
    }

    /// Ends the line being redrawn, so the next output starts clean.
    fn finish(&mut self) {
        if self.open {
            println!();
            self.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_follow_the_plan() {
        let installing = Update::Installing {
            percent: Some(42),
            done: 14 * 1024 * 1024 * 1024,
            total: 33 * 1024 * 1024 * 1024,
        };
        assert_eq!(
            line(Action::Install, "The Witcher 3", &Update::Waiting).unwrap(),
            "Waiting for Steam's install dialog..."
        );
        assert_eq!(
            line(Action::Install, "The Witcher 3", &installing).unwrap(),
            "Installing The Witcher 3... 42% (14 GiB of 33 GiB)"
        );
        let no_totals = Update::Installing {
            percent: None,
            done: 0,
            total: 0,
        };
        assert_eq!(
            line(Action::Install, "The Witcher 3", &no_totals).unwrap(),
            "Installing The Witcher 3..."
        );
        assert_eq!(
            line(Action::Uninstall, "The Witcher 3", &Update::Uninstalling).unwrap(),
            "Uninstalling The Witcher 3..."
        );
        assert_eq!(line(Action::Install, "x", &Update::Finished), None);
    }
}
