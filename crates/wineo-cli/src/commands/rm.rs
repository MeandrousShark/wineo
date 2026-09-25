use std::fs;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, bail};
use wineo_core::config::GlobalConfig;
use wineo_core::environment::{self, Environment};
use wineo_core::operation::{Locks, Operation};
use wineo_core::paths::Paths;
use wineo_core::prefix;
use wineo_core::runtime::Runtime;
use wineo_core::{art, desktop};

use super::transfer;

#[derive(clap::Args)]
pub struct Args {
    /// Environment to remove
    #[arg(add = clap_complete::ArgValueCompleter::new(crate::completion::environments))]
    name: String,

    /// Delete the environment's wine prefix as well
    #[arg(long)]
    delete_prefix: bool,
}

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let name = &args.name;
    Environment::ensure_exists(paths, name)?;
    // A broken config must not block removal, so the prefix stays optional.
    let environment = Environment::load(paths, name).ok();
    let prefix = environment
        .as_ref()
        .and_then(|environment| environment.prefix(paths).ok());

    let locks = Locks::new(paths);
    let held: Vec<&Path> = prefix.as_deref().into_iter().collect();
    let _guard = locks.acquire(Operation::Remove, name, &held)?;
    // Everything so far was read before the lock, so it only counts now.
    Environment::ensure_exists(paths, name)?;

    let runtime = Runtime::new(paths, name);
    if runtime.is_running()? {
        bail!("{name} is running, stop it first");
    }
    match runtime.status() {
        // A missing supervisor lock doesn't prove wine stopped.
        Ok(status) if status.lost => bail!(
            "{name}'s last run was lost, so wine may still be running in its prefix. Clean it up with `wineo stop {name}` first"
        ),
        Err(err) if args.delete_prefix => bail!(
            "{name}'s run state can't be read, so wine may still be running in its prefix. Clean it up with `wineo stop {name}` first: {err}"
        ),
        _ => {}
    }
    runtime.check_resolved(name)?;

    if args.delete_prefix {
        let Some(prefix) = &prefix else {
            bail!(
                "{name}'s config can't be read, so its prefix can't be found. Remove it without --delete-prefix and delete the prefix yourself"
            );
        };
        delete_prefix(paths, name, prefix)?;
    }

    // The config goes last, so a failure always leaves the environment registered.
    let dir = runtime.dir();
    if dir.exists() {
        fs::remove_dir_all(dir)
            .with_context(|| format!("failed to remove {}, {name} was kept", dir.display()))?;
    }
    let config = paths.environment_config(name);
    let config_guard = locks.config(&config)?;
    fs::remove_file(&config).with_context(|| format!("failed to remove {}", config.display()))?;
    drop(config_guard);

    // After the config, so a failure here cannot leave an environment that is
    // half removed. A leftover entry is listed by `wineo desktop` as a stray.
    let removed = desktop::remove_all(paths, name)?;
    if !removed.is_empty() {
        eprintln!(
            "Removed {} desktop entr{}",
            removed.len(),
            if removed.len() == 1 { "y" } else { "ies" }
        );
    }
    if art::remove_all(paths, name)? {
        eprintln!("Removed art");
    }

    match (&environment, args.delete_prefix) {
        (_, true) => println!("Removed environment {name} and its prefix"),
        (Some(environment), false) => println!(
            "Removed environment {name}, its prefix is still at {}",
            paths.shown(&environment.config.prefix)
        ),
        (None, false) => println!("Removed environment {name}; its prefix was left untouched"),
    }
    if let Some(prefix) = &prefix {
        let snapshots = paths.snapshot_root(prefix, name);
        let has_snapshots = match fs::read_dir(&snapshots) {
            Ok(mut entries) => entries.next().is_some(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => fs::symlink_metadata(&snapshots).is_ok(),
        };
        if has_snapshots {
            println!("Snapshots remain at {}", snapshots.display());
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn delete_prefix(paths: &Paths, name: &str, prefix: &Path) -> anyhow::Result<()> {
    let shown = paths
        .contract(prefix)
        .unwrap_or_else(|_| prefix.display().to_string());
    // A dangling symlink still counts as present, so it is cleaned up too.
    if fs::symlink_metadata(prefix).is_err() {
        eprintln!("The prefix at {shown} is already gone");
        return Ok(());
    }

    // Deleting the link would leave the prefix behind, and deleting what it
    // points at is exactly the following this avoids everywhere else.
    if fs::symlink_metadata(prefix).is_ok_and(|meta| meta.is_symlink()) {
        bail!(
            "{shown} is a symlink, so it was not deleted. Delete what it points at yourself, then run `wineo rm {name}` to remove the registration"
        );
    }

    let sharing = environment::sharing_prefix(paths, name, prefix)?;
    if !sharing.names.is_empty() {
        bail!(
            "{shown} is also used by {}, so it was not deleted. Remove {name} without --delete-prefix instead",
            sharing.names.join(", ")
        );
    }
    if !sharing.unreadable.is_empty() {
        bail!(
            "{} can't be read, so it's unknown whether {shown} is still in use. Fix or remove {} first, or remove {name} without --delete-prefix",
            sharing.unreadable.join(", "),
            if sharing.unreadable.len() == 1 {
                "that config"
            } else {
                "those configs"
            }
        );
    }
    // The runner may be gone or the config broken, so either layout counts.
    if !prefix::holds_prefix(prefix) {
        bail!(
            "{shown} is not a wine prefix, so it was not deleted. Look at what is there, clean it up yourself if it's what's left of a partly deleted prefix, then run `wineo rm {name}` to remove the registration"
        );
    }

    // A broken global config must not block removal, so waiting is skipped then.
    if let Ok(global) = GlobalConfig::load(paths) {
        transfer::wait_idle(paths, &global, name)?;
    }

    // remove_dir_all removes symlinks rather than following them, so a prefix's
    // links to `/` and the home directory can't take anything else with them.
    fs::remove_dir_all(prefix).with_context(|| {
        format!("failed to delete the prefix at {shown}, {name} was kept and the prefix may be partly deleted")
    })
}
