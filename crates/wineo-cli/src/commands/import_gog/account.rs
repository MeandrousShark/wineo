use std::process::{Command, ExitCode, Stdio};

use wineo_core::gog::account;
use wineo_core::gog::api::{Client, Endpoints};
use wineo_core::key_store::Saved;
use wineo_core::paths::Paths;

use crate::commands::gog_support::Account;
use crate::prompt;
use crate::style;

/// Opens GOG's login page and logs in with the address of the page the
/// browser lands on. The address holds the one-time login code, so it is
/// read without echo and never printed or kept.
pub(super) fn login(paths: &Paths) -> anyhow::Result<ExitCode> {
    anyhow::ensure!(prompt::interactive(), "Logging in to GOG needs a terminal.");
    let client = wineo_core::gog::client_from_env()?;
    let url = client.login_url();
    println!("Log in to GOG in your browser. If it doesn't open, go to:");
    println!("{url}");
    open_browser(&client, &url);
    println!("Then copy the address of the page you land on.");
    let Some(pasted) = prompt::secret("Address: ")?.filter(|pasted| !pasted.is_empty()) else {
        anyhow::bail!("No address entered.");
    };
    let account = Account::new();
    let logged_in = account::login(paths, account.keyring(), client, &pasted)?;
    println!("Logged in to GOG as account {}.", logged_in.user_id);
    if let Saved::File(path) = &logged_in.saved {
        eprintln!(
            "{}",
            style::warning_stderr(format!(
                "warning: no keyring, so the login is stored in plain text in {}",
                path.display()
            ))
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// Best effort: the address is printed too, for a terminal over SSH.
fn open_browser(client: &Client, url: &str) {
    // A fake server in a test isn't GOG, and a browser on it is just noise.
    if client.endpoints() != &Endpoints::production() {
        return;
    }
    let _ = Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

pub(super) fn logout(paths: &Paths) -> anyhow::Result<ExitCode> {
    let account = Account::new();
    let out = account::logout(paths, account.keyring())?;
    if !(out.removed.keyring || out.removed.file) {
        println!("No GOG login was saved.");
        return Ok(ExitCode::SUCCESS);
    }
    println!("Removed the GOG login.");
    if out.cache_removed
        && let Some(user_id) = out.user_id
    {
        println!("Removed the owned-games list for account {user_id}.");
    }
    Ok(ExitCode::SUCCESS)
}
