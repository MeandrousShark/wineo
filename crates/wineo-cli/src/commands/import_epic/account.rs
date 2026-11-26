use std::process::{Command, ExitCode, Stdio};

use wineo_core::config::GlobalConfig;
use wineo_core::epic::account::{self, LoginCode};
use wineo_core::paths::Paths;

use super::setup::{self, Found};
use super::{Args, HOST_ONLY};
use crate::commands::transfer::Cancellation;
use crate::prompt;

/// The page that logs in to Epic as Legendary and then shows the code.
const LOGIN_URL: &str = "https://legendary.gl/epiclogin";

/// Opens Epic's login page and logs in with what the page shows. That holds
/// a one-time login code, so it is read without echo and never printed.
pub(super) fn login(paths: &Paths, args: &Args) -> anyhow::Result<ExitCode> {
    anyhow::ensure!(
        args.dry_run || prompt::interactive(),
        "Logging in to Epic needs a terminal."
    );
    let global = GlobalConfig::load(paths)?;
    let Some(legendary) = setup::obtain(paths, &global, args)? else {
        return Ok(ExitCode::SUCCESS);
    };
    if args.dry_run {
        println!("Dry run, nothing was changed.");
        return Ok(ExitCode::SUCCESS);
    }
    println!("Log in to Epic in your browser. If it doesn't open, go to:");
    println!("{LOGIN_URL}");
    open_browser(LOGIN_URL);
    println!("Then paste what the page shows, or only its authorizationCode.");
    let Some(pasted) = prompt::secret("Code: ")?.filter(|pasted| !pasted.is_empty()) else {
        anyhow::bail!("No code entered.");
    };
    let code = LoginCode::parse(&pasted)?;
    let cancellation = Cancellation::new()?;
    let logged_in = account::login(&legendary, &code, &cancellation.token);
    let name = cancellation.result(logged_in.map_err(anyhow::Error::from))?;
    println!("Logged in to Epic as {name}.");
    Ok(ExitCode::SUCCESS)
}

/// Best effort: the address is printed too, for a terminal over SSH.
fn open_browser(url: &str) {
    let _ = Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Never asks to download Legendary. Without one to run, wineo removes
/// Legendary's saved login itself, which is all Legendary's logout does.
pub(super) fn logout(paths: &Paths) -> anyhow::Result<ExitCode> {
    anyhow::ensure!(paths.portable_root().is_none(), HOST_ONLY);
    let global = GlobalConfig::load(paths)?;
    let was = match setup::find(paths, &global)? {
        Found::Usable(legendary) => {
            let cancellation = Cancellation::new()?;
            let out = account::logout(&legendary, &cancellation.token);
            cancellation.result(out.map_err(anyhow::Error::from))?
        }
        Found::Unusable { .. } => account::logout_without_legendary(paths)?,
    };
    println!(
        "{}",
        if was {
            "Removed the Epic login."
        } else {
            "No Epic login was saved."
        }
    );
    Ok(ExitCode::SUCCESS)
}
