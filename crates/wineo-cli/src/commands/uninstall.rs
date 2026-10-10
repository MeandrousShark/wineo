use std::process::ExitCode;

use wineo_core::paths::Paths;
use wineo_core::steam_install::Action;

#[derive(clap::Args)]
pub struct Args {
    /// Steam game to uninstall
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: String,

    /// Stay attached and show progress until Steam is done
    #[arg(short, long)]
    follow: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    super::install::act(paths, &args.name, args.follow, Action::Uninstall)
}
