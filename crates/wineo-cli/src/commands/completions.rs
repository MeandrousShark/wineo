use std::io;
use std::process::ExitCode;

use clap_complete::Shell;

#[derive(clap::Args)]
pub struct Args {
    /// Shell to generate the script for
    shell: Shell,
}

/// Saved scripts go stale after an upgrade, so they have to be regenerated
/// whenever commands or flags change.
pub fn run(command: &mut clap::Command, args: Args) -> anyhow::Result<ExitCode> {
    let name = command.get_name().to_owned();
    clap_complete::generate(args.shell, command, name, &mut io::stdout());
    Ok(ExitCode::SUCCESS)
}
