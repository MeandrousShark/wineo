use std::io::Write;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use wineo_core::config::GlobalConfig;
use wineo_core::download::{Client, Progress, Provider, Release, Transport};
use wineo_core::paths::Paths;

use super::transfer::Cancellation;
use crate::cancel;
use crate::prompt::{self, Choice, Input, Prompt, Reply, Terminal, numbered};
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
        Err(error) if cancel::prompted(&error) => {
            eprintln!("{}", style::warning_stderr("Download canceled"));
            Ok(cancel::exit_code())
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
                list_to(
                    &client,
                    provider,
                    args.page,
                    verbose,
                    &cancellation,
                    &mut crate::output::stdout(),
                    &mut crate::output::stderr(),
                )?;
                return Ok(ExitCode::SUCCESS);
            }
            let release = cancellation.result(client.select(provider, args.version.as_deref()))?;
            eprintln!(
                "{}",
                describe(paths, &global, &release, style::stderr_enabled())?
            );
            install(paths, &global, &client, &release, &cancellation).map(|_| ())
        }
        None => {
            anyhow::ensure!(
                prompt::interactive(),
                "guided download needs a terminal; use `wineo download <provider> [--version <tag>]`"
            );
            let mut prompt = Prompt::new(Terminal::open()?, crate::output::stdout())
                .with_color(style::stdout_enabled());
            choose_and_install(paths, &global, &mut prompt, &PROVIDERS, verbose).map(|_| ())
        }
    };
    match result {
        Err(error) if cancel::prompted(&error) => {
            eprintln!("{}", style::warning_stderr("Download canceled"));
            Ok(cancel::exit_code())
        }
        other => other.map(|()| ExitCode::SUCCESS),
    }
}

fn list_to<T: Transport, W: Write, E: Write>(
    client: &Client<T>,
    provider: Provider,
    page: u32,
    verbose: bool,
    cancellation: &Cancellation,
    output: &mut W,
    notices: &mut E,
) -> anyhow::Result<()> {
    let catalog = cancellation.result(client.list(provider, page))?;
    for release in catalog.releases {
        writeln!(output, "{}", row(&release, verbose))?;
    }
    if catalog.has_more {
        writeln!(
            notices,
            "{}",
            style::dim_stderr(format!(
                "More releases: wineo download {provider} --list --page {}",
                page + 1
            ))
        )?;
    }
    Ok(())
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
        let client = Client::new()?;
        match pick_release(paths, global, prompt, provider, &client, verbose)? {
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
fn pick_release<I: Input, W: Write, T: Transport>(
    paths: &Paths,
    global: &GlobalConfig,
    prompt: &mut Prompt<I, W>,
    provider: Provider,
    client: &Client<T>,
    verbose: bool,
) -> anyhow::Result<Reply<String>> {
    loop {
        let catalog = match listed(client, provider, 1) {
            Ok(catalog) => catalog,
            Err(error) if cancel::prompted(&error) => return Err(error),
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
                match pick_from_full_list(paths, global, prompt, provider, client, verbose)? {
                    Reply::Back => continue,
                    Reply::Value(name) => return Ok(Reply::Value(name)),
                }
            }
            Reply::Value(Pick::Release(index)) => {
                let release = &top[index];
                prompt.say(describe(paths, global, release, style::stdout_enabled())?)?;
                if !matches!(
                    prompt.confirm("Download this release?", true)?,
                    Reply::Value(true)
                ) {
                    continue;
                }
                match installed(paths, global, client, release) {
                    Ok(name) => return Ok(Reply::Value(name)),
                    Err(error) if cancel::prompted(&error) => return Err(error),
                    Err(error) => {
                        prompt.say(style::error(format!("Download failed: {error:#}")))?;
                    }
                }
            }
        }
    }
}

/// The complete, paged release list, reached from `pick_release` with `m`.
fn pick_from_full_list<I: Input, W: Write, T: Transport>(
    paths: &Paths,
    global: &GlobalConfig,
    prompt: &mut Prompt<I, W>,
    provider: Provider,
    client: &Client<T>,
    verbose: bool,
) -> anyhow::Result<Reply<String>> {
    let mut page = 1;
    loop {
        let catalog = match listed(client, provider, page) {
            Ok(catalog) => catalog,
            Err(error) if cancel::prompted(&error) => return Err(error),
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
                prompt.say(describe(paths, global, release, style::stdout_enabled())?)?;
                if !matches!(
                    prompt.confirm("Download this release?", true)?,
                    Reply::Value(true)
                ) {
                    continue;
                }
                match installed(paths, global, client, release) {
                    Ok(name) => return Ok(Reply::Value(name)),
                    Err(error) if cancel::prompted(&error) => return Err(error),
                    Err(error) => {
                        prompt.say(style::error(format!("Download failed: {error:#}")))?;
                    }
                }
            }
        }
    }
}

// The handler is scoped to each network step rather than the whole guided
// flow, so Ctrl+C at a prompt takes the prompt's own path.
fn listed<T: Transport>(
    client: &Client<T>,
    provider: Provider,
    page: u32,
) -> anyhow::Result<wineo_core::download::Catalog> {
    let cancellation = Cancellation::new()?;
    cancellation.result(client.list(provider, page))
}

fn installed<T: Transport>(
    paths: &Paths,
    global: &GlobalConfig,
    client: &Client<T>,
    release: &Release,
) -> anyhow::Result<String> {
    let cancellation = Cancellation::new()?;
    install(paths, global, client, release, &cancellation)
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

fn describe(
    paths: &Paths,
    global: &GlobalConfig,
    release: &Release,
    color: bool,
) -> anyhow::Result<String> {
    Ok(format!(
        "{}\n  asset: {} ({})\n  verification: {}\n  destination: {}",
        style::title_with_color(&release.tag, color),
        release.asset_name,
        style::size(release.size),
        release.verification(),
        style::path_with_color(paths, &release.destination(paths, global)?, color)
    ))
}

/// Also used by `import heroic --all` to fetch a GE-Proton build when no
/// Proton runner is installed.
pub(crate) fn install<T: Transport>(
    paths: &Paths,
    global: &GlobalConfig,
    client: &Client<T>,
    release: &Release,
    cancellation: &Cancellation,
) -> anyhow::Result<String> {
    let mut last = Instant::now() - Duration::from_secs(1);
    let result =
        client.install(
            paths,
            global,
            release,
            &cancellation.token,
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
                    eprintln!("Installed in {}", style::path_stderr(paths, &path))
                }
                Progress::AlreadyInstalled(path) => {
                    eprintln!("Already installed in {}", style::path_stderr(paths, &path))
                }
                _ => {}
            },
        );
    cancellation.published_result(result)?;
    if matches!(release.provider, Provider::Dxvk | Provider::Vkd3d) {
        eprintln!("Environments set to `latest` pick this up at their next reconciliation.");
    } else {
        eprintln!("New and unpinned environments can select this release.");
    }
    Ok(release.installed_name.clone())
}

/// Downloads the newest stable release for `provider`, with no picking: for
/// callers that already decided what they want and just need it fetched,
/// like a missing Proton runner or DXVK/vkd3d-proton build offered a quick
/// download instead of the full picker.
pub(crate) fn install_latest(
    paths: &Paths,
    global: &GlobalConfig,
    provider: Provider,
) -> anyhow::Result<String> {
    let cancellation = Cancellation::new()?;
    let client = Client::new()?;
    install_selected(
        paths,
        global,
        Selection {
            provider,
            tag: None,
            expected_name: None,
        },
        &client,
        &cancellation,
        &mut crate::output::stderr(),
    )
}

pub(crate) fn install_component_version(
    paths: &Paths,
    global: &GlobalConfig,
    provider: Provider,
    installed_name: &str,
) -> anyhow::Result<String> {
    let cancellation = Cancellation::new()?;
    let client = Client::new()?;
    install_selected(
        paths,
        global,
        Selection {
            provider,
            tag: None,
            expected_name: Some(installed_name),
        },
        &client,
        &cancellation,
        &mut crate::output::stderr(),
    )
}

struct Selection<'a> {
    provider: Provider,
    tag: Option<&'a str>,
    expected_name: Option<&'a str>,
}

fn install_selected<T: Transport, W: Write>(
    paths: &Paths,
    global: &GlobalConfig,
    selection: Selection<'_>,
    client: &Client<T>,
    cancellation: &Cancellation,
    output: &mut W,
) -> anyhow::Result<String> {
    let release = cancellation.result(match selection.expected_name {
        Some(installed_name) => client.select_pinned(selection.provider, installed_name),
        None => client.select(selection.provider, selection.tag),
    })?;
    writeln!(
        output,
        "{}",
        describe(paths, global, &release, style::stderr_enabled())?
    )?;
    install(paths, global, client, &release, cancellation)
}

#[cfg(test)]
mod tests;
