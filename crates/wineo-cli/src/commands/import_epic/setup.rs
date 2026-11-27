//! Finding Legendary, and the question that downloads wineo's own copy when
//! there is none to use.

use wineo_core::cancel::Token;
use wineo_core::config::GlobalConfig;
use wineo_core::epic::legendary::{self, Legendary, Missing};
use wineo_core::epic::{self, FLOOR, own_copy};
use wineo_core::paths::Paths;

use super::Args;
use crate::commands::transfer::Cancellation;
use crate::prompt::{self, Canceled, Prompt, Reply, Terminal};

const NEEDS_YES: &str = "Downloading Legendary needs a terminal, or --yes to agree to it.";

pub(crate) enum Found {
    Usable(Box<Legendary>),
    /// None to run. `missing` says what an offer to download names, and is
    /// None when wineo's own copy is there but Python can't run it.
    Unusable {
        missing: Option<Missing>,
    },
}

/// Looks for Legendary without ever asking to download it. A setting that
/// can't be used stays an error.
pub(crate) fn find(paths: &Paths, global: &GlobalConfig) -> anyhow::Result<Found> {
    let cancellation = Cancellation::new()?;
    let found = legendary::find(paths, global, &cancellation.token);
    match cancellation.result(found.map_err(anyhow::Error::from)) {
        Ok(legendary::Found::Usable(legendary)) => Ok(Found::Usable(legendary)),
        Ok(legendary::Found::Missing(missing)) => Ok(Found::Unusable {
            missing: Some(missing),
        }),
        Err(error) if matches!(error.downcast_ref(), Some(epic::Error::Python)) => {
            Ok(Found::Unusable { missing: None })
        }
        Err(error) => Err(error),
    }
}

/// A Legendary to run, downloading wineo's own copy first when the user
/// agrees. None after a dry run that stopped at the question.
pub(crate) fn obtain(
    paths: &Paths,
    global: &GlobalConfig,
    args: &Args,
) -> anyhow::Result<Option<Box<Legendary>>> {
    match find(paths, global)? {
        Found::Usable(legendary) => Ok(Some(legendary)),
        Found::Unusable { missing: None } => Err(epic::Error::Python.into()),
        Found::Unusable {
            missing: Some(missing),
        } => {
            if !download(paths, &missing, args)? {
                return Ok(None);
            }
            match find(paths, global)? {
                Found::Usable(legendary) => Ok(Some(legendary)),
                Found::Unusable { missing: None } => Err(epic::Error::Python.into()),
                Found::Unusable { missing: Some(_) } => {
                    anyhow::bail!("Couldn't find the Legendary wineo just downloaded.")
                }
            }
        }
    }
}

/// The question itself.
pub(super) fn question() -> String {
    // Rounded to the nearest, so 3,093,622 bytes reads as the plan's 3 MB.
    let megabytes = ((own_copy::PINNED.size + 500_000) / 1_000_000).max(1);
    format!(
        "Epic needs Legendary {} ({megabytes} MB, from its GitHub releases). Download it?",
        own_copy::PINNED.version
    )
}

/// What a dry run says in place of asking.
pub(crate) fn say_dry_run(missing: &Missing) {
    say_older(missing);
    eprintln!("{}", question());
    eprintln!("Dry run, so wineo didn't ask or download anything.");
}

fn say_older(missing: &Missing) {
    if let Some((path, version)) = &missing.older_on_path {
        eprintln!(
            "The legendary on PATH ({}) is {version}, older than {FLOOR}.",
            path.display()
        );
    }
}

/// Fails as offering the download would: on another machine, or without a
/// new enough Python. A dry run checks this too, to fail where the real
/// run would.
pub(crate) fn check_can_download() -> anyhow::Result<()> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err(epic::Error::Unsupported.into());
    }
    legendary::python(&Token::new())?;
    Ok(())
}

/// Asks, then downloads. False after a dry run. Answering no is a cancel.
/// Nothing is offered that couldn't run: on another machine, or without a
/// new enough Python, that is the error instead.
fn download(paths: &Paths, missing: &Missing, args: &Args) -> anyhow::Result<bool> {
    check_can_download()?;
    if args.dry_run {
        say_dry_run(missing);
        return Ok(false);
    }
    if !args.yes {
        anyhow::ensure!(prompt::interactive(), NEEDS_YES);
        say_older(missing);
        // On stderr, so `--list --json` keeps stdout for its JSON.
        let mut prompt =
            Prompt::new(Terminal::open()?, crate::output::stderr()).with_first_question(true);
        match prompt.confirm(&question(), true)? {
            Reply::Value(true) => {}
            Reply::Value(false) | Reply::Back => return Err(Canceled.into()),
        }
    }
    let version = own_copy::PINNED.version;
    let cancellation = Cancellation::new()?;
    // Said once the first bytes arrive, so a copy another wineo fetched
    // meanwhile isn't announced as a download.
    let mut started = false;
    let fetched = own_copy::get(paths, &cancellation.token, |_, _| {
        if !std::mem::replace(&mut started, true) {
            eprintln!("Downloading Legendary {version}...");
        }
    });
    let fetched = cancellation.result(fetched.map_err(anyhow::Error::from))?;
    if matches!(fetched, own_copy::Fetched::Downloaded(_)) {
        eprintln!("Downloaded Legendary {version}.");
    }
    Ok(true)
}
