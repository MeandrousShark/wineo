//! `wineo import gog`: the GOG login, the games you own, installing them,
//! and moving the games Heroic installed into wineo.

use std::process::ExitCode;

use wineo_core::config::Arch;
use wineo_core::gog::folder::{FolderError, Platform};

use crate::prompt::Canceled;

mod account;
mod extras;
mod install;
mod list;
mod migrate;

#[derive(clap::Args, Default)]
#[command(group = clap::ArgGroup::new("mode").args(["login", "logout", "list", "forget", "game", "all"]))]
pub struct Args {
    /// Log in to GOG in your browser
    #[arg(long)]
    pub login: bool,
    /// Remove the saved GOG login and its owned-games list
    #[arg(long)]
    pub logout: bool,
    /// List the games you own on GOG
    #[arg(long)]
    pub list: bool,
    /// With --list, fetch the owned games again instead of using the saved list
    #[arg(long, requires = "list")]
    pub refresh: bool,
    /// With --list, print JSON
    #[arg(long, requires = "list")]
    pub json: bool,
    /// Forget the saved state of a game folder that no environment owns, or
    /// of an install that stopped before its environment was created
    #[arg(long, value_name = "PATH", value_hint = clap::ValueHint::DirPath)]
    pub forget: Option<String>,
    /// Install a game by GOG id, or migrate it when Heroic has it
    #[arg(long, value_name = "ID")]
    pub game: Option<String>,
    /// With --game, the platform to install for, Linux when GOG has a Linux installer
    #[arg(long, value_enum, requires = "game")]
    pub platform: Option<InstallPlatform>,
    /// With --game, the language to install in, a locale tag like fr-FR
    #[arg(long, value_name = "TAG", requires = "game")]
    pub language: Option<String>,
    /// With --game, the runner a Windows game's environment gets
    #[arg(long, value_name = "NAME", requires = "game")]
    pub runner: Option<String>,
    /// With --game, a Windows game's prefix architecture, win64 or win32
    #[arg(long, value_name = "ARCH", requires = "game")]
    pub arch: Option<Arch>,
    /// With --game, the folder to install into instead of the games folder
    #[arg(long, value_name = "PATH", value_hint = clap::ValueHint::DirPath, requires = "game")]
    pub dir: Option<String>,
    /// Migrate every game Heroic installed
    #[arg(long)]
    pub all: bool,
    /// Skip the confirmation and allow running without a terminal
    #[arg(short, long, conflicts_with_all = ["login", "logout", "list", "forget"])]
    pub yes: bool,
    /// Review the install or migration without changing anything
    #[arg(long, conflicts_with_all = ["login", "logout", "list", "forget"])]
    pub dry_run: bool,
}

/// What `--platform` takes.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum InstallPlatform {
    Linux,
    Windows,
}

impl From<InstallPlatform> for Platform {
    fn from(platform: InstallPlatform) -> Self {
        match platform {
            InstallPlatform::Linux => Platform::Linux,
            InstallPlatform::Windows => Platform::Windows,
        }
    }
}

pub fn run(
    paths: &wineo_core::paths::Paths,
    args: Args,
    verbose: bool,
    first_question: bool,
) -> anyhow::Result<ExitCode> {
    let result = if args.login {
        account::login(paths)
    } else if args.logout {
        account::logout(paths)
    } else if args.list {
        list::run(paths, &args)
    } else if let Some(folder) = &args.forget {
        migrate::forget(paths, folder)
    } else if args.all {
        migrate::all(paths, &args, verbose)
    } else if let Some(id) = &args.game {
        migrate::one(paths, &args, id, verbose)
    } else {
        migrate::guided(paths, &args, first_question, verbose)
    };
    match result {
        Err(error) if canceled(&error) => {
            eprintln!("Import canceled.");
            Ok(ExitCode::from(130))
        }
        other => other,
    }
}

fn canceled(error: &anyhow::Error) -> bool {
    error.is::<Canceled>()
        || matches!(
            error.downcast_ref::<FolderError>(),
            Some(FolderError::Canceled)
        )
        || error.is::<wineo_core::import::Cancelled>()
        || matches!(
            error.downcast_ref::<wineo_core::Error>(),
            Some(wineo_core::Error::Canceled(_))
        )
}
