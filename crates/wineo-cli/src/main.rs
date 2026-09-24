mod commands;
mod completion;
mod picker;
mod prompt;
mod style;
mod wizard;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand, ValueHint};
use wineo_core::components::Component;
use wineo_core::paths::Paths;
use wineo_core::supervisor;

#[derive(Parser)]
#[command(name = "wineo", version, about = "Manage Wine environments")]
struct Cli {
    /// Show full detail that lists normally leave out, like installed names
    #[arg(short, long, global = true)]
    verbose: bool,
    /// Keep config and prefixes in DIR instead of the standard locations
    #[arg(long, global = true, value_name = "DIR", value_hint = ValueHint::DirPath)]
    portable: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create an environment, asking for what it needs unless given a name
    New(commands::new::Args),
    /// Copy an environment and its prefix under a new name
    Clone(commands::clone::Args),
    /// Copy an environment into a portable folder
    Portable(commands::portable::Args),
    /// Save or restore a local environment snapshot
    Snapshot(commands::snapshot::Args),
    /// Change settings of an environment, or the global config
    Set(commands::set::Args),
    /// Pick, edit, and remove an environment's targets, or run an installer in it
    Setup(commands::setup::Args),
    /// Print an environment's config, or the global config
    Config(commands::config::Args),
    /// Start an environment in the background
    Start(commands::start::Args),
    /// Stop a running environment
    Stop(commands::stop::Args),
    /// Stop an environment if it's running, then start it again
    Restart(commands::restart::Args),
    /// List installed wine runners
    Runners(commands::runners::Args),
    /// Download a runner or graphics component
    Download(commands::download::Args),
    /// Copy or adopt a prefix, with optional Heroic launch settings
    Import(commands::import::Args),
    /// List installed DXVK builds
    Dxvk(commands::builds::Args),
    /// List installed vkd3d-proton builds
    Vkd3d(commands::builds::Args),
    /// List the templates `wineo new` can start from
    Templates(commands::templates::Args),
    /// List the programs installed in an environment
    Programs(commands::programs::Args),
    /// List the desktop entries wineo has written
    Desktop(commands::desktop::Args),
    /// Set or remove an environment's artwork
    Art(commands::art::Args),
    /// Remove an environment
    Rm(commands::rm::Args),
    /// Show whether environments are running
    Status(commands::status::Args),
    /// List environments and their state
    List(commands::list::Args),
    /// Print the output of an environment's detached runs
    Logs(commands::logs::Args),
    /// Run an environment attached to the terminal
    Run(commands::run::Args),
    /// Run another program inside an environment
    Exec(commands::exec::Args),
    /// Run winetricks in an environment
    Winetricks(commands::winetricks::Args),
    /// Print a shell completion script
    Completions(commands::completions::Args),
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{}", style::error_stderr(format!("error: {err:#}")));
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<ExitCode> {
    // Owns a detached run for `start`. Deliberately not a clap subcommand, so
    // it stays out of help and generated completion scripts.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == supervisor::SUPERVISE_COMMAND)
    {
        return supervisor::serve()
            .map(|()| ExitCode::SUCCESS)
            .map_err(Into::into);
    }

    clap_complete::CompleteEnv::with_factory(Cli::command).complete();

    let cli = Cli::parse();
    let paths = Paths::from_env(cli.portable.as_deref())?;
    let verbose = cli.verbose;
    match cli.command {
        Command::New(args) => commands::new::run(&paths, args),
        Command::Clone(args) => commands::clone::run(&paths, args, verbose),
        Command::Portable(args) => commands::portable::run(&paths, args, verbose),
        Command::Snapshot(args) => commands::snapshot::run(&paths, args, verbose),
        Command::Set(args) => commands::set::run(&paths, args),
        Command::Setup(args) => commands::setup::run(&paths, args),
        Command::Config(args) => commands::config::run(&paths, args),
        Command::Start(args) => commands::start::run(&paths, args),
        Command::Stop(args) => commands::stop::run(&paths, args),
        Command::Restart(args) => commands::restart::run(&paths, args),
        Command::Import(args) => commands::import::run(&paths, args, verbose),
        Command::Download(args) => commands::download::run(&paths, args, verbose),
        Command::Runners(args) => commands::runners::run(&paths, args),
        Command::Dxvk(args) => commands::builds::run(&paths, Component::Dxvk, args),
        Command::Vkd3d(args) => commands::builds::run(&paths, Component::Vkd3d, args),
        Command::Templates(args) => commands::templates::run(&paths, args),
        Command::Programs(args) => commands::programs::run(&paths, args),
        Command::Desktop(args) => commands::desktop::run(&paths, args),
        Command::Art(args) => commands::art::run(&paths, args),
        Command::Rm(args) => commands::rm::run(&paths, args),
        Command::Status(args) => commands::status::run(&paths, args),
        Command::List(args) => commands::list::run(&paths, args),
        Command::Logs(args) => commands::logs::run(&paths, args),
        Command::Run(args) => commands::run::run(&paths, args),
        Command::Exec(args) => commands::exec::run(&paths, args),
        Command::Winetricks(args) => commands::winetricks::run(&paths, args),
        Command::Completions(args) => commands::completions::run(&mut Cli::command(), args),
    }
}
