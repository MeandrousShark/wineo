//! `wineo import epic`: the Epic login and the games you own, through
//! Legendary, which wineo can download for you the first time Epic needs it.

use std::process::ExitCode;

use wineo_core::config::{Arch, GlobalConfig};
use wineo_core::epic;
use wineo_core::paths::Paths;

use crate::cancel;

mod account;
pub(crate) mod install;
pub(crate) mod list;
pub(crate) mod setup;

/// The modes a `--game` flag conflicts with. clap skips `requires` when an
/// argument that conflicts with it through the `mode` group is given, so
/// they are named outright.
const NOT_GAME: [&str; 4] = ["login", "logout", "list", "forget"];

#[derive(clap::Args, Default)]
#[command(group = clap::ArgGroup::new("mode").args(["login", "logout", "list", "game", "forget"]))]
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
    #[arg(long, requires = "list", conflicts_with_all = ["login", "logout", "game", "forget"])]
    pub refresh: bool,
    /// With --list, print JSON
    #[arg(long, requires = "list", conflicts_with_all = ["login", "logout", "game", "forget"])]
    pub json: bool,
    /// Install a game by its Epic id, or finish an install that didn't finish
    #[arg(long, value_name = "ID")]
    pub game: Option<String>,
    /// Forget a game no environment owns, leaving its files
    #[arg(long, value_name = "ID")]
    pub forget: Option<String>,
    /// With --game, the runner the game's environment gets
    #[arg(long, value_name = "NAME", requires = "game", conflicts_with_all = NOT_GAME)]
    pub runner: Option<String>,
    /// With --game, the prefix architecture, win64 or win32
    #[arg(long, value_name = "ARCH", requires = "game", conflicts_with_all = NOT_GAME)]
    pub arch: Option<Arch>,
    /// With --game, the folder to install into instead of the games folder
    #[arg(
        long,
        value_name = "PATH",
        value_hint = clap::ValueHint::DirPath,
        requires = "game",
        conflicts_with_all = NOT_GAME
    )]
    pub dir: Option<String>,
    /// Skip the questions, agreeing to download Legendary, and allow running without a terminal
    #[arg(short, long, conflicts_with_all = ["logout", "forget"])]
    pub yes: bool,
    /// Say what would happen without downloading Legendary or changing anything
    #[arg(long, conflicts_with_all = ["logout", "forget"])]
    pub dry_run: bool,
}

/// What every command that isn't logging in or listing says in a portable
/// root.
const HOST_ONLY: &str = epic::install::HOST_ONLY;

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let result = if args.login {
        account::login(paths, &args)
    } else if args.logout {
        account::logout(paths)
    } else if args.list {
        list::run(paths, &args)
    } else if let Some(app) = &args.game {
        install::one(paths, &args, app)
    } else if let Some(app) = &args.forget {
        install::forget(paths, app)
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
        || epic::install::is_cancel(error)
        || error
            .downcast_ref::<epic::Error>()
            .is_some_and(epic::Error::is_canceled)
}
