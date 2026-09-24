use std::path::PathBuf;
use std::process::ExitCode;

use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{Subcommand, ValueHint};
use clap_complete::ArgValueCompleter;
use wineo_core::art::{self, Kind};
use wineo_core::paths::Paths;

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
        }
        Command::Rm { spot, kind } => {
            let (env, target) = split(&spot);
            art::remove(paths, env, target, kind)?;
        }
    }
    Ok(ExitCode::SUCCESS)
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
