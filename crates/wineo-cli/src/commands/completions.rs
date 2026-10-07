use std::io::Write;
use std::process::ExitCode;

use clap_complete::Shell;

#[derive(clap::Args)]
pub struct Args {
    /// Shell to generate the script for
    shell: Shell,
}

pub fn run(command: &mut clap::Command, args: Args) -> anyhow::Result<ExitCode> {
    match args.shell {
        Shell::Bash => println!("source <(COMPLETE=bash wineo)"),
        Shell::Fish => println!("COMPLETE=fish wineo | source"),
        Shell::Zsh => println!(
            "#compdef wineo\nsource <(COMPLETE=zsh wineo)\nif [[ -n ${{CURRENT-}} ]]; then\n  _clap_dynamic_completer_wineo \"$@\"\nfi"
        ),
        shell => {
            let name = command.get_name().to_owned();
            let mut script = Vec::new();
            clap_complete::generate(shell, command, name, &mut script);
            crate::output::stdout().write_all(&script)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}
