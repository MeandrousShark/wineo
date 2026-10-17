//! Old spellings of flags and subcommands that keep working until 2.0.
//!
//! The old spelling is a hidden clap alias, which the generated completion
//! scripts leave out but which parses to the same thing, so clap can't say
//! which one was typed. `warn_used` walks the raw args against the command
//! tree to find out, and prints the warning on stderr, so `--json` stdout
//! and the exit code are unaffected.
//!
//! To retire another spelling: add the hidden alias next to the new name,
//! then add a row to `RENAMED`.

use std::ffi::OsString;

use clap::Command;

use crate::style;

/// What changed at a command, named by its current path.
enum Change {
    /// A flag of the command at `path` was renamed.
    Flag {
        old: &'static str,
        new: &'static str,
    },
    /// The command at `path` was renamed, from `old` in the same parent.
    Command { old: &'static str },
}

struct Renamed {
    path: &'static [&'static str],
    change: Change,
}

const RENAMED: &[Renamed] = &[
    Renamed {
        path: &["rm"],
        change: Change::Flag {
            old: "delete-prefix",
            new: "include-prefix",
        },
    },
    Renamed {
        path: &["snapshot", "remove"],
        change: Change::Command { old: "delete" },
    },
];

/// Warns once for each old spelling in `args`.
pub fn warn_used(root: &Command, args: impl IntoIterator<Item = OsString>) {
    let typed = walk(root, args);
    for renamed in RENAMED {
        if let Some(line) = line(renamed, &typed) {
            eprintln!("{}", style::warning_stderr(line));
        }
    }
}

fn line(renamed: &Renamed, typed: &Typed) -> Option<String> {
    let depth = renamed.path.len();
    let reached = typed.commands.get(..depth)?;
    if !reached
        .iter()
        .map(|(canonical, _)| canonical.as_str())
        .eq(renamed.path.iter().copied())
    {
        return None;
    }
    match renamed.change {
        Change::Flag { old, new } => typed
            .flags
            .iter()
            .any(|(at, flag)| *at == depth && flag == old)
            .then(|| format!("warning: --{old} is deprecated; use --{new}")),
        Change::Command { old } => {
            let (_, spelling) = &reached[depth - 1];
            let parent = renamed.path[..depth - 1].join(" ");
            (spelling == old).then(|| {
                format!(
                    "warning: {parent} {old} is deprecated; use {}",
                    renamed.path.join(" ")
                )
            })
        }
    }
}

/// What the args named: each subcommand reached as (canonical name, spelling
/// typed), and each long flag as (how many subcommands were reached when it
/// appeared, name).
#[derive(Default)]
struct Typed {
    commands: Vec<(String, String)>,
    flags: Vec<(usize, String)>,
}

fn walk(root: &Command, args: impl IntoIterator<Item = OsString>) -> Typed {
    let mut typed = Typed::default();
    // Flags are looked up from every command on the way down, since globals
    // are declared on an ancestor.
    let mut path: Vec<&Command> = vec![root];
    let mut args = args.into_iter().skip(1);
    while let Some(arg) = args.next() {
        let Some(arg) = arg.to_str() else { continue };
        if arg == "--" {
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, attached) = match long.split_once('=') {
                Some((name, _)) => (name, true),
                None => (long, false),
            };
            typed.flags.push((typed.commands.len(), name.to_owned()));
            let takes_value = path
                .iter()
                .flat_map(|command| command.get_arguments())
                .find(|flag| {
                    flag.get_long() == Some(name)
                        || flag
                            .get_all_aliases()
                            .is_some_and(|all| all.contains(&name))
                })
                .is_some_and(|flag| flag.get_action().takes_values());
            if takes_value && !attached {
                args.next();
            }
        } else if let Some(shorts) = arg.strip_prefix('-').filter(|rest| !rest.is_empty()) {
            for (index, short) in shorts.char_indices() {
                let takes_value = path
                    .iter()
                    .flat_map(|command| command.get_arguments())
                    .find(|flag| flag.get_short() == Some(short))
                    .is_some_and(|flag| flag.get_action().takes_values());
                if takes_value {
                    if index + short.len_utf8() == shorts.len() {
                        args.next();
                    }
                    break;
                }
            }
        } else if let Some(sub) = path.last().and_then(|command| command.find_subcommand(arg)) {
            typed
                .commands
                .push((sub.get_name().to_owned(), arg.to_owned()));
            path.push(sub);
        }
    }
    typed
}

#[cfg(test)]
mod tests {
    use clap::{Arg, ArgAction};

    use super::*;

    fn tree() -> Command {
        Command::new("wineo")
            .arg(
                Arg::new("portable")
                    .long("portable")
                    .global(true)
                    .action(ArgAction::Set),
            )
            .arg(
                Arg::new("verbose")
                    .short('v')
                    .long("verbose")
                    .global(true)
                    .action(ArgAction::SetTrue),
            )
            .subcommand(
                Command::new("rm")
                    .visible_alias("remove")
                    .arg(Arg::new("name"))
                    .arg(
                        Arg::new("include")
                            .long("include-prefix")
                            .alias("delete-prefix")
                            .action(ArgAction::SetTrue),
                    ),
            )
            .subcommand(
                Command::new("snapshot")
                    .subcommand(
                        Command::new("remove")
                            .visible_alias("rm")
                            .alias("delete")
                            .args([Arg::new("env"), Arg::new("label")]),
                    )
                    .subcommand(Command::new("list").arg(Arg::new("env"))),
            )
    }

    fn warned(args: &[&str]) -> Vec<String> {
        let typed = walk(&tree(), args.iter().map(OsString::from));
        RENAMED
            .iter()
            .filter_map(|renamed| line(renamed, &typed))
            .collect()
    }

    #[test]
    fn old_flag_warns_through_the_alias_and_with_a_value() {
        let line = "warning: --delete-prefix is deprecated; use --include-prefix";
        assert_eq!(warned(&["wineo", "rm", "a", "--delete-prefix"]), [line]);
        assert_eq!(warned(&["wineo", "remove", "--delete-prefix", "a"]), [line]);
        assert_eq!(
            warned(&["wineo", "-v", "rm", "--delete-prefix", "a"]),
            [line]
        );
        assert!(warned(&["wineo", "rm", "a", "--include-prefix"]).is_empty());
        assert!(warned(&["wineo", "rm", "a", "--", "--delete-prefix"]).is_empty());
    }

    #[test]
    fn old_subcommand_warns_but_a_value_with_the_same_text_does_not() {
        let line = "warning: snapshot delete is deprecated; use snapshot remove";
        assert_eq!(warned(&["wineo", "snapshot", "delete", "g", "one"]), [line]);
        assert_eq!(
            warned(&[
                "wineo",
                "--portable",
                "/x",
                "snapshot",
                "delete",
                "g",
                "one"
            ]),
            [line]
        );
        assert!(warned(&["wineo", "snapshot", "remove", "g", "one"]).is_empty());
        assert!(warned(&["wineo", "snapshot", "rm", "g", "delete"]).is_empty());
        assert!(warned(&["wineo", "snapshot", "list", "delete"]).is_empty());
        assert!(
            warned(&[
                "wineo",
                "--portable",
                "delete",
                "snapshot",
                "rm",
                "g",
                "one"
            ])
            .is_empty()
        );
        assert!(
            warned(&[
                "wineo",
                "-v",
                "--portable=delete",
                "snapshot",
                "rm",
                "g",
                "x"
            ])
            .is_empty()
        );
    }
}
