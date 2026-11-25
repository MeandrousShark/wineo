//! `wineo import epic`: the Epic login and the games you own, through
//! Legendary, which wineo can download for you the first time Epic needs it.

use std::process::ExitCode;

use wineo_core::config::GlobalConfig;
use wineo_core::epic;
use wineo_core::paths::Paths;

use crate::cancel;

mod account;
mod list;
mod setup;

#[derive(clap::Args, Default)]
#[command(group = clap::ArgGroup::new("mode").args(["login", "logout", "list"]))]
pub struct Args {
    /// Log in to Epic in your browser
    #[arg(long)]
    pub login: bool,
    /// Log out of Epic and remove the owned-games list
    #[arg(long)]
    pub logout: bool,
    /// List the games you own on Epic
    #[arg(long)]
    pub list: bool,
    /// With --list, fetch the owned games again instead of using the saved list
    #[arg(long, requires = "list")]
    pub refresh: bool,
    /// With --list, print JSON
    #[arg(long, requires = "list")]
    pub json: bool,
    /// Agree to downloading Legendary, and allow it without a terminal
    #[arg(short, long, conflicts_with = "logout")]
    pub yes: bool,
    /// Say what would happen without downloading Legendary or changing anything
    #[arg(long, conflicts_with = "logout")]
    pub dry_run: bool,
}

/// What every command that isn't logging in or listing says in a portable
/// root.
const HOST_ONLY: &str = "Epic games live in the host library. Run this without --portable.";

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let result = if args.login {
        account::login(paths, &args)
    } else if args.logout {
        account::logout(paths)
    } else if args.list {
        list::run(paths, &args)
    } else {
        guided(paths, &args)
    };
    match result {
        Err(error) if canceled(&error) => {
            eprintln!("Import canceled.");
            Ok(cancel::exit_code())
        }
        other => other,
    }
}

/// Bare `wineo import epic`: logs in when logged out, else lists what the
/// account owns.
fn guided(paths: &Paths, args: &Args) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let logged_in = match setup::find(paths, &global)? {
        setup::Found::Usable(legendary) => {
            let cancellation = crate::commands::transfer::Cancellation::new()?;
            let status = epic::account::status(&legendary, &cancellation.token);
            matches!(
                cancellation.result(status.map_err(anyhow::Error::from))?,
                epic::account::Status::LoggedIn { .. }
            )
        }
        // No Legendary has no login to use either.
        setup::Found::Unusable { .. } => false,
    };
    if logged_in {
        list::run(paths, args)
    } else {
        account::login(paths, args)
    }
}

fn canceled(error: &anyhow::Error) -> bool {
    cancel::prompted(error)
        || cancel::core(error)
        || error
            .downcast_ref::<epic::Error>()
            .is_some_and(epic::Error::is_canceled)
}
