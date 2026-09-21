use std::io::{self, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use wineo_core::config::GlobalConfig;
use wineo_core::download::{Client, Progress, Provider, Release};
use wineo_core::paths::Paths;

use super::transfer::Cancellation;
use crate::prompt::{self, Canceled, Choice, Input, Prompt, Reply, Terminal, numbered};

pub const PROVIDERS: [Provider; 4] = [
    Provider::GeProton,
    Provider::ProtonCachyos,
    Provider::Dxvk,
    Provider::Vkd3d,
];
pub const RUNNERS: [Provider; 2] = [Provider::GeProton, Provider::ProtonCachyos];

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

pub fn run(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    match run_inner(paths, args) {
        Err(error) if error.is::<Canceled>() => {
            eprintln!("Download canceled");
            Ok(ExitCode::from(130))
        }
        other => other,
    }
}

fn run_inner(paths: &Paths, args: Args) -> anyhow::Result<ExitCode> {
    let global = GlobalConfig::load(paths)?;
    let result = match args.provider {
        Some(provider) => {
            let cancellation = Cancellation::new()?;
            let client = Client::new()?;
            if args.list {
                let catalog = cancellation.result(client.list(provider, args.page))?;
                for release in catalog.releases {
                    println!(
                        "{}  {}  {} bytes  {}",
                        release.tag, release.installed_name, release.size, release.asset_name
                    );
                }
                if catalog.has_more {
                    eprintln!(
                        "More releases: wineo download {provider} --list --page {}",
                        args.page + 1
                    );
                }
                return Ok(ExitCode::SUCCESS);
            }
            let release = cancellation.result(client.select(provider, args.version.as_deref()))?;
            install(paths, &global, &client, &release, &cancellation).map(|_| ())
        }
        None => {
            anyhow::ensure!(
                prompt::interactive(),
                "guided download needs a terminal; use `wineo download <provider> [--version <tag>]`"
            );
            let mut prompt = Prompt::new(Terminal::open()?, io::stdout());
            choose_and_install(paths, &global, &mut prompt, &PROVIDERS).map(|_| ())
        }
    };
    match result {
        Err(error) if error.is::<Canceled>() => {
            eprintln!("Download canceled");
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
        let mut page = 1;
        loop {
            let catalog = match cancellation.result(client.list(provider, page)) {
                Ok(catalog) => catalog,
                Err(error) if error.is::<Canceled>() => return Err(error),
                Err(error) => {
                    prompt.say(format!("Couldn't list releases: {error:#}"))?;
                    break;
                }
            };
            #[derive(Clone)]
            enum Pick {
                Release(usize),
                Next,
                Previous,
            }
            let mut choices = numbered(catalog.releases.iter().enumerate().map(|(i, r)| {
                (
                    format!("{} ({} bytes, {})", r.tag, r.size, r.installed_name),
                    Pick::Release(i),
                )
            }));
            if catalog.has_more {
                choices.push(Choice::letter('n', "Next page", Pick::Next));
            }
            if page > 1 {
                choices.push(Choice::letter('p', "Previous page", Pick::Previous));
            }
            match prompt.choose(
                &format!("Available {provider} releases (page {page})"),
                "Release",
                &choices,
                None,
                true,
            )? {
                Reply::Back => break,
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
                    match install(paths, global, &client, release, &cancellation) {
                        Ok(name) => return Ok(Reply::Value(name)),
                        Err(error) if error.is::<Canceled>() => return Err(error),
                        Err(error) => prompt.say(format!("Download failed: {error:#}"))?,
                    }
                }
            }
        }
        if providers.len() == 1 {
            return Ok(Reply::Back);
        }
    }
}

fn describe(paths: &Paths, global: &GlobalConfig, release: &Release) -> anyhow::Result<String> {
    Ok(format!(
        "{}\n  asset: {} ({} bytes)\n  verification: {}\n  destination: {}",
        release.tag,
        release.asset_name,
        release.size,
        release.verification(),
        release.destination(paths, global)?.display()
    ))
}

fn install(
    paths: &Paths,
    global: &GlobalConfig,
    client: &Client,
    release: &Release,
    cancellation: &Cancellation,
) -> anyhow::Result<String> {
    eprintln!("{}", describe(paths, global, release)?);
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
                    eprintln!("Downloaded {received} / {total:?} bytes");
                    last = Instant::now();
                }
                Progress::Verifying => eprintln!("Verifying download"),
                Progress::Extracting => eprintln!("Extracting build"),
                Progress::Installed(path) => eprintln!("Installed in {}", path.display()),
                Progress::AlreadyInstalled(path) => {
                    eprintln!("Already installed in {}", path.display())
                }
                _ => {}
            },
        );
    cancellation.result(result)?;
    if matches!(release.provider, Provider::Dxvk | Provider::Vkd3d) {
        eprintln!(
            "Environments using this component's `latest` setting will select the newest local build at their next reconciliation."
        );
    } else {
        eprintln!(
            "Runner aliases and new gaming environments can select this release. Pinned environments keep their runner."
        );
    }
    Ok(release.installed_name.clone())
}
