use std::io::{self, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use wineo_core::config::GlobalConfig;
use wineo_core::download::{Client, Progress, Provider, Release};
use wineo_core::paths::Paths;

use super::transfer::Cancellation;
use crate::prompt::{self, Canceled, Choice, Input, Prompt, Reply, Terminal, numbered};
use crate::style;

pub const PROVIDERS: [Provider; 4] = [
    Provider::GeProton,
    Provider::ProtonCachyos,
    Provider::Dxvk,
    Provider::Vkd3d,
];
pub const RUNNERS: [Provider; 2] = [Provider::GeProton, Provider::ProtonCachyos];

/// How many releases to list before letting go to `more`.
const SHORT_LIST: usize = 3;

#[derive(clap::Args)]
pub struct Args {
    /// ge-proton, proton-cachyos, dxvk, or vkd3d; omit for guided selection
    provider: Option<Provider>,
    /// List available stable releases without downloading
    #[arg(long, requires = "provider", conflicts_with = "version")]
    list: bool,
    /// Release listing page
    #[arg(long, default_value_t = 1, requires = "list", value_parser = clap::value_parser!(u32).range(1..))]
    page: u32,
    /// Exact upstream release tag
    #[arg(long, requires = "provider")]
    version: Option<String>,
}

pub fn run(paths: &Paths, args: Args, verbose: bool) -> anyhow::Result<ExitCode> {
    match run_inner(paths, args, verbose) {
        Err(error) if error.is::<Canceled>() => {
            eprintln!("{}", style::warning("Download canceled"));
            Ok(ExitCode::from(130))
        }
        other => other,
    }
}

fn run_inner(paths: &Paths, args: Args, verbose: bool) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let result = match args.provider {
        Some(provider) => {
            let cancellation = Cancellation::new()?;
            let client = Client::new()?;
            if args.list {
                let catalog = cancellation.result(client.list(provider, args.page))?;
                for release in catalog.releases {
                    println!("{}", row(&release, verbose));
                }
                if catalog.has_more {
                    eprintln!(
                        "{}",
                        style::dim(format!(
                            "More releases: wineo download {provider} --list --page {}",
                            args.page + 1
                        ))
                    );
                }
                return Ok(ExitCode::SUCCESS);
            }
            let release = cancellation.result(client.select(provider, args.version.as_deref()))?;
            eprintln!("{}", describe(paths, &global, &release)?);
            install(paths, &global, &client, &release, &cancellation).map(|_| ())
        }
        None => {
            anyhow::ensure!(
                prompt::interactive(),
                "guided download needs a terminal; use `wineo download <provider> [--version <tag>]`"
            );
            let mut prompt =
                Prompt::new(Terminal::open()?, io::stdout()).with_color(style::enabled());
            choose_and_install(paths, &global, &mut prompt, &PROVIDERS, verbose).map(|_| ())
        }
    };
    match result {
        Err(error) if error.is::<Canceled>() => {
            eprintln!("{}", style::warning("Download canceled"));
            Ok(ExitCode::from(130))
        }
        other => other.map(|()| ExitCode::SUCCESS),
    }
}

pub fn choose_and_install<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    prompt: &mut Prompt<I, W>,
    providers: &[Provider],
    verbose: bool,
) -> anyhow::Result<Reply<String>> {
    loop {
        let provider = if providers.len() == 1 {
            providers[0]
        } else {
            match prompt.choose(
                "What would you like to download?",
                "Tool",
                &numbered(
                    providers
                        .iter()
                        .map(|provider| (provider.to_string(), *provider)),
                ),
                Some("1"),
                true,
            )? {
                Reply::Back => return Ok(Reply::Back),
                Reply::Value(provider) => provider,
            }
        };
        let cancellation = Cancellation::new()?;
        let client = Client::new()?;
        match pick_release(
            paths,
            global,
            prompt,
            provider,
            &client,
            &cancellation,
            verbose,
        )? {
            Reply::Back => {
                if providers.len() == 1 {
                    return Ok(Reply::Back);
                }
            }
            Reply::Value(name) => return Ok(Reply::Value(name)),
        }
    }
}

/// Shows the newest few releases, with `m` opening the full paged list.
fn pick_release<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    prompt: &mut Prompt<I, W>,
    provider: Provider,
    client: &Client,
    cancellation: &Cancellation,
    verbose: bool,
) -> anyhow::Result<Reply<String>> {
    loop {
        let catalog = match cancellation.result(client.list(provider, 1)) {
            Ok(catalog) => catalog,
            Err(error) if error.is::<Canceled>() => return Err(error),
            Err(error) => {
                prompt.say(style::error(format!("Couldn't list releases: {error:#}")))?;
                return Ok(Reply::Back);
            }
        };
        #[derive(Clone)]
        enum Pick {
            Release(usize),
            More,
        }
        let top = &catalog.releases[..catalog.releases.len().min(SHORT_LIST)];
        let mut choices = numbered(top.iter().enumerate().map(|(index, release)| {
            let label = if index == 0 {
                format!("{}  [latest]", row(release, verbose))
            } else {
                row(release, verbose)
            };
            (label, Pick::Release(index))
        }));
        choices.push(Choice::letter('m', "more", Pick::More));
        match prompt.choose(
            &format!("{provider} releases:"),
            "Release",
            &choices,
            Some("1"),
            true,
        )? {
            Reply::Back => return Ok(Reply::Back),
            Reply::Value(Pick::More) => {
                match pick_from_full_list(
                    paths,
                    global,
                    prompt,
                    provider,
                    client,
                    cancellation,
                    verbose,
                )? {
                    Reply::Back => continue,
                    Reply::Value(name) => return Ok(Reply::Value(name)),
                }
            }
            Reply::Value(Pick::Release(index)) => {
                let release = &top[index];
                prompt.say(describe(paths, global, release)?)?;
                if !matches!(
                    prompt.confirm("Download this release?", true)?,
                    Reply::Value(true)
                ) {
                    continue;
                }
                match install(paths, global, client, release, cancellation) {
                    Ok(name) => return Ok(Reply::Value(name)),
                    Err(error) if error.is::<Canceled>() => return Err(error),
                    Err(error) => {
                        prompt.say(style::error(format!("Download failed: {error:#}")))?;
                    }
                }
            }
        }
    }
}

/// The complete, paged release list, reached from `pick_release` with `m`.
fn pick_from_full_list<I: Input, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    prompt: &mut Prompt<I, W>,
    provider: Provider,
    client: &Client,
    cancellation: &Cancellation,
    verbose: bool,
) -> anyhow::Result<Reply<String>> {
    let mut page = 1;
    loop {
        let catalog = match cancellation.result(client.list(provider, page)) {
            Ok(catalog) => catalog,
            Err(error) if error.is::<Canceled>() => return Err(error),
            Err(error) => {
                prompt.say(style::error(format!("Couldn't list releases: {error:#}")))?;
                return Ok(Reply::Back);
            }
        };
        #[derive(Clone)]
        enum Pick {
            Release(usize),
            Next,
            Previous,
        }
        let mut choices = numbered(
            catalog
                .releases
                .iter()
                .enumerate()
                .map(|(i, r)| (row(r, verbose), Pick::Release(i))),
        );
        if catalog.has_more {
            choices.push(Choice::letter('n', "Next page", Pick::Next));
        }
        if page > 1 {
            choices.push(Choice::letter('p', "Previous page", Pick::Previous));
        }
        match prompt.choose(
            &format!("{provider} releases (page {page}):"),
            "Release",
            &choices,
            None,
            true,
        )? {
            Reply::Back => return Ok(Reply::Back),
            Reply::Value(Pick::Next) => page += 1,
            Reply::Value(Pick::Previous) => page -= 1,
            Reply::Value(Pick::Release(index)) => {
                let release = &catalog.releases[index];
                prompt.say(describe(paths, global, release)?)?;
                if !matches!(
                    prompt.confirm("Download this release?", true)?,
                    Reply::Value(true)
                ) {
                    continue;
                }
                match install(paths, global, client, release, cancellation) {
                    Ok(name) => return Ok(Reply::Value(name)),
                    Err(error) if error.is::<Canceled>() => return Err(error),
                    Err(error) => {
                        prompt.say(style::error(format!("Download failed: {error:#}")))?;
                    }
                }
            }
        }
    }
}

/// A release's listing row: tag and size, with the installed name added by
/// `-v`.
fn row(release: &Release, verbose: bool) -> String {
    release_row(&release.tag, release.size, &release.installed_name, verbose)
}

fn release_row(tag: &str, size: u64, installed_name: &str, verbose: bool) -> String {
    if verbose {
        format!("{tag}  {}  {installed_name}", style::size(size))
    } else {
        format!("{tag}  {}", style::size(size))
    }
}

fn describe(paths: &Paths, global: &GlobalConfig, release: &Release) -> anyhow::Result<String> {
    Ok(format!(
        "{}\n  asset: {} ({})\n  verification: {}\n  destination: {}",
        style::title(&release.tag),
        release.asset_name,
        style::size(release.size),
        release.verification(),
        style::path(paths, &release.destination(paths, global)?)
    ))
}

fn install(
    paths: &Paths,
    global: &GlobalConfig,
    client: &Client,
    release: &Release,
    cancellation: &Cancellation,
) -> anyhow::Result<String> {
    let mut last = Instant::now() - Duration::from_secs(1);
    let result =
        client.install(
            paths,
            global,
            release,
            &cancellation.flag,
            |progress| match progress {
                Progress::Downloading { received, total }
                    if last.elapsed() >= Duration::from_secs(1) =>
                {
                    eprintln!(
                        "Downloaded {} / {}",
                        style::size(received),
                        style::size(total)
                    );
                    last = Instant::now();
                }
                Progress::Verifying => eprintln!("Verifying download"),
                Progress::Extracting => eprintln!("Extracting build"),
                Progress::Installed(path) => {
                    eprintln!("Installed in {}", style::path(paths, &path))
                }
                Progress::AlreadyInstalled(path) => {
                    eprintln!("Already installed in {}", style::path(paths, &path))
                }
                _ => {}
            },
        );
    cancellation.result(result)?;
    if matches!(release.provider, Provider::Dxvk | Provider::Vkd3d) {
        eprintln!("Environments set to `latest` pick this up at their next reconciliation.");
    } else {
        eprintln!("New and unpinned environments can select this release.");
    }
    Ok(release.installed_name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_shows_tag_and_size_only_by_default() {
        assert_eq!(
            release_row(
                "cachyos-11.0-20260703-slr",
                320 * 1024 * 1024,
                "proton-cachyos-11.0-20260703-slr-x86_64",
                false
            ),
            "cachyos-11.0-20260703-slr  320 MiB"
        );
    }

    #[test]
    fn verbose_adds_the_installed_name() {
        assert_eq!(
            release_row(
                "cachyos-11.0-20260703-slr",
                320 * 1024 * 1024,
                "proton-cachyos-11.0-20260703-slr-x86_64",
                true
            ),
            "cachyos-11.0-20260703-slr  320 MiB  proton-cachyos-11.0-20260703-slr-x86_64"
        );
    }
}
