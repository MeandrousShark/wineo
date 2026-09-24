use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{Subcommand, ValueHint};
use clap_complete::ArgValueCompleter;
use wineo_core::art::{self, Kind, Listing, Resolved, Source, Warning};
use wineo_core::paths::Paths;

use crate::style;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Copy an image into the store as an environment's or a target's art
    Set {
        #[arg(value_name = "ENV[.TARGET]", add = ArgValueCompleter::new(crate::completion::spots))]
        spot: String,
        #[arg(value_parser = kind_parser())]
        kind: Kind,
        #[arg(value_hint = ValueHint::FilePath)]
        file: PathBuf,
    },
    /// Show an environment's art and its targets' overrides
    List {
        #[arg(add = ArgValueCompleter::new(crate::completion::environments))]
        env: String,
    },
    /// Remove one piece of art
    Rm {
        #[arg(value_name = "ENV[.TARGET]", add = ArgValueCompleter::new(crate::completion::spots))]
        spot: String,
        #[arg(value_parser = kind_parser())]
        kind: Kind,
    },
}

fn kind_parser() -> impl TypedValueParser<Value = Kind> {
    PossibleValuesParser::new(Kind::ALL.map(Kind::as_str))
        .map(|kind| kind.parse().expect("only a possible value gets here"))
}

/// Environment names never contain a dot, so the first one starts the target.
fn split(spot: &str) -> (&str, Option<&str>) {
    match spot.split_once('.') {
        Some((env, target)) => (env, Some(target)),
        None => (spot, None),
    }
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    match args.command {
        Command::Set { spot, kind, file } => {
            let (env, target) = split(&spot);
            art::set_manual(paths, env, target, kind, &file)?;
            println!("Set {kind} for {spot}");
        }
        Command::List { env } => {
            let listing = art::list(paths, &env)?;
            let (lines, warnings) = render(&env, &paths.art_dir(&env), &listing);
            for line in lines {
                println!("{line}");
            }
            for warning in warnings {
                eprintln!("{}", style::warning_stderr(format!("warning: {warning}")));
            }
        }
        Command::Rm { spot, kind } => {
            let (env, target) = split(&spot);
            art::remove(paths, env, target, kind)?;
            println!("Removed {kind} from {spot}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The lines `art list` prints, and its warnings.
fn render(env: &str, art_dir: &Path, listing: &Listing) -> (Vec<String>, Vec<String>) {
    let file = |resolved: &Resolved| {
        resolved
            .found
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let width = listing
        .kinds
        .iter()
        .filter_map(|(_, resolved)| resolved.as_ref())
        .chain(
            listing
                .targets
                .iter()
                .flat_map(|(_, overrides)| overrides.iter().map(|(_, resolved)| resolved)),
        )
        .map(|resolved| file(resolved).len())
        .max()
        .unwrap_or(0);
    let row = |kind: Kind, resolved: &Resolved| {
        format!(
            "  {kind}  {:<width$}  {}",
            file(resolved),
            source(resolved.source.as_ref())
        )
    };

    let mut lines = vec![match listing.game {
        Some(game) => format!("{env} (SteamGridDB game {game})"),
        None => env.to_owned(),
    }];
    for (kind, resolved) in &listing.kinds {
        lines.push(match resolved {
            Some(resolved) => row(*kind, resolved),
            None => format!("  {kind}  none"),
        });
    }
    for (target, overrides) in &listing.targets {
        lines.push(format!("{env}.{target}"));
        lines.extend(
            overrides
                .iter()
                .map(|(kind, resolved)| row(*kind, resolved)),
        );
    }

    let shown = |path: &Path| {
        path.strip_prefix(art_dir)
            .unwrap_or(path)
            .display()
            .to_string()
    };
    let mut warnings = Vec::new();
    for warning in &listing.warnings {
        match warning {
            Warning::Duplicates {
                used,
                ignored,
                recorded,
                ..
            } => {
                let reason = if *recorded {
                    "is recorded"
                } else {
                    "takes precedence"
                };
                for path in ignored {
                    warnings.push(format!(
                        "{} is ignored because {} {reason}",
                        shown(path),
                        shown(used)
                    ));
                }
            }
            Warning::MissingFile { path, .. } => {
                warnings.push(format!(
                    "art.toml records {}, which is missing",
                    shown(path)
                ));
            }
            Warning::UnknownTarget(target) => warnings.push(format!(
                "art.toml has art for target `{target}`, which {env} doesn't have"
            )),
        }
    }
    (lines, warnings)
}

fn source(source: Option<&Source>) -> String {
    match source {
        Some(Source::SteamGridDb(asset)) => format!("SteamGridDB {asset}"),
        Some(Source::Manual(original)) => format!("manual, from {original}"),
        None => "unrecorded".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::split;

    #[test]
    fn a_spot_splits_at_its_first_dot() {
        assert_eq!(split("hades"), ("hades", None));
        assert_eq!(split("hades.server"), ("hades", Some("server")));
        assert_eq!(split("hades.a.b"), ("hades", Some("a.b")));
        assert_eq!(split("hades."), ("hades", Some("")));
    }
}
