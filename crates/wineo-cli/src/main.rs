// Rust printing macros panic on closed pipes. These shadows handle output errors
// quietly and are defined before the modules so they cover every CLI module.
macro_rules! print {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let mut stdout = std::io::stdout().lock();
        crate::output::handle(write!(stdout, $($arg)*));
        crate::output::handle(stdout.flush());
    }};
}

macro_rules! println {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        crate::output::handle(writeln!(std::io::stdout().lock(), $($arg)*));
    }};
}

macro_rules! eprint {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        crate::output::handle(write!(std::io::stderr().lock(), $($arg)*));
    }};
}

macro_rules! eprintln {
    () => { eprint!("\n") };
    ($($arg:tt)*) => { eprint!("{}\n", format_args!($($arg)*)) };
}

mod cancel;
mod commands;
mod completion;
mod output;
mod picker;
mod prompt;
mod style;
mod text;
mod wizard;

// Shared with the integration tests, which include the same file.
#[cfg(test)]
#[path = "../tests/common/snapshot.rs"]
mod snapshot;
#[cfg(test)]
#[path = "../../wineo-core/tests/common/test_script.rs"]
mod test_script;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand, ValueHint};
use wineo_core::components::Component;
use wineo_core::deprecated;
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
    /// Ask Steam to install a Steam game
    Install(commands::install::Args),
    /// Ask Steam to uninstall a Steam game
    Uninstall(commands::uninstall::Args),
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
    /// Check GOG for updates to the games wineo manages
    Update(commands::update::Args),
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
    /// Return a trash entry's data to its original name and paths
    Restore(commands::restore::Args),
    /// List recoverable environment removals
    Trash(commands::trash::Args),
    /// Permanently delete selected trash entries
    Prune(commands::prune::Args),
    /// Show whether environments are running
    Status(commands::status::Args),
    /// Show playtime
    Stats(commands::stats::Args),
    /// List environments and their state
    List(commands::list::Args),
    /// Check configs and leftover state, and fix what is safe to fix
    Validate(commands::validate::Args),
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
    /// Write man pages for wineo and its commands into a folder
    Manpages(commands::manpages::Args),
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => report_error(&err),
    }
}

fn report_error(error: &anyhow::Error) -> ExitCode {
    eprintln!("{}", style::error_stderr(error_message(error)));
    ExitCode::FAILURE
}

fn error_message(error: &anyhow::Error) -> String {
    format!("error: {error:#}")
}

fn run() -> anyhow::Result<ExitCode> {
    // Owns a detached run for `start`. Deliberately not a clap subcommand, so
    // it stays out of help and generated completion scripts.
    if let Some(result) = supervisor::dispatch_if_supervise() {
        return result.map(|()| ExitCode::SUCCESS).map_err(Into::into);
    }

    let completion = clap_complete::CompleteEnv::with_factory(Cli::command)
        .try_complete(std::env::args_os(), std::env::current_dir().ok().as_deref());
    match completion {
        Ok(true) => return Ok(ExitCode::SUCCESS),
        Ok(false) => {}
        Err(error) => {
            if output::is_clap_broken_pipe(&error) {
                return Ok(ExitCode::from(output::CLOSED_PIPE));
            }
            output::handle(error.print());
            return Ok(ExitCode::from(error.exit_code() as u8));
        }
    }

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            output::handle(error.print());
            return Ok(ExitCode::from(error.exit_code() as u8));
        }
    };
    let paths = Paths::from_env(cli.portable.as_deref())?;
    let verbose = cli.verbose;
    if !matches!(
        cli.command,
        Command::Validate(_) | Command::Completions(_) | Command::Manpages(_)
    ) {
        warn_deprecated(&paths);
    }
    match cli.command {
        Command::New(args) => commands::new::run(&paths, args),
        Command::Clone(args) => commands::clone::run(&paths, args, verbose),
        Command::Portable(args) => commands::portable::run(&paths, args, verbose),
        Command::Snapshot(args) => commands::snapshot::run(&paths, args, verbose),
        Command::Set(args) => commands::set::run(&paths, args),
        Command::Setup(args) => commands::setup::run(&paths, args),
        Command::Config(args) => commands::config::run(&paths, args),
        Command::Start(args) => commands::start::run(&paths, args),
        Command::Install(args) => commands::install::run(&paths, args),
        Command::Uninstall(args) => commands::uninstall::run(&paths, args),
        Command::Stop(args) => commands::stop::run(&paths, args),
        Command::Restart(args) => commands::restart::run(&paths, args),
        Command::Import(args) => commands::import::run(&paths, args, verbose),
        Command::Update(args) => commands::update::run(&paths, args),
        Command::Download(args) => commands::download::run(&paths, args, verbose),
        Command::Runners(args) => commands::runners::run(&paths, args),
        Command::Dxvk(args) => commands::builds::run(&paths, Component::Dxvk, args),
        Command::Vkd3d(args) => commands::builds::run(&paths, Component::Vkd3d, args),
        Command::Templates(args) => commands::templates::run(&paths, args),
        Command::Programs(args) => commands::programs::run(&paths, args),
        Command::Desktop(args) => commands::desktop::run(&paths, args),
        Command::Art(args) => commands::art::run(&paths, args),
        Command::Rm(args) => commands::rm::run(&paths, args),
        Command::Restore(args) => commands::restore::run(&paths, args),
        Command::Trash(args) => commands::trash::run(&paths, args),
        Command::Prune(args) => commands::prune::run(&paths, args),
        Command::Status(args) => commands::status::run(&paths, args),
        Command::Stats(args) => commands::stats::run(&paths, args),
        Command::List(args) => commands::list::run(&paths, args),
        Command::Validate(args) => commands::validate::run(&paths, args, verbose),
        Command::Logs(args) => commands::logs::run(&paths, args),
        Command::Run(args) => commands::run::run(&paths, args),
        Command::Exec(args) => commands::exec::run(&paths, args),
        Command::Winetricks(args) => commands::winetricks::run(&paths, args),
        Command::Completions(args) => commands::completions::run(&mut Cli::command(), args),
        Command::Manpages(args) => commands::manpages::run(Cli::command(), args),
    }
}

/// One line for every config a deprecated key is in, before the command
/// runs, so a long `run` shows it up front. `validate` lists each key itself.
fn warn_deprecated(paths: &Paths) {
    if let Some(line) = deprecated::summary(deprecated::count_configs(paths)) {
        eprintln!("{}", style::warning_stderr(format!("warning: {line}")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_output_broken_pipe_keeps_the_error_exit_code() {
        let error = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
            .context("child pipe or socket failed");
        assert_eq!(report_error(&error), ExitCode::FAILURE);
        assert_eq!(
            error_message(&error),
            "error: child pipe or socket failed: broken pipe"
        );
    }
}
