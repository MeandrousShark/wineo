use std::io::{self, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use wineo_core::config::GlobalConfig;
use wineo_core::download::{Client, Progress, Provider, Release, Transport};
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
            eprintln!("{}", style::warning_stderr("Download canceled"));
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
                list_to(
                    &client,
                    provider,
                    args.page,
                    verbose,
                    &cancellation,
                    &mut io::stdout(),
                    &mut io::stderr(),
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
            let mut prompt =
                Prompt::new(Terminal::open()?, io::stdout()).with_color(style::enabled());
            choose_and_install(paths, &global, &mut prompt, &PROVIDERS, verbose).map(|_| ())
        }
    };
    match result {
        Err(error) if error.is::<Canceled>() => {
            eprintln!("{}", style::warning_stderr("Download canceled"));
            Ok(ExitCode::from(130))
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
fn pick_release<I: Input, W: Write, T: Transport>(
    paths: &Paths,
    global: &GlobalConfig,
    prompt: &mut Prompt<I, W>,
    provider: Provider,
    client: &Client<T>,
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
                prompt.say(describe(paths, global, release, style::stdout_enabled())?)?;
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
fn pick_from_full_list<I: Input, W: Write, T: Transport>(
    paths: &Paths,
    global: &GlobalConfig,
    prompt: &mut Prompt<I, W>,
    provider: Provider,
    client: &Client<T>,
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
                prompt.say(describe(paths, global, release, style::stdout_enabled())?)?;
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
        &mut io::stderr(),
    )
}

pub(crate) fn install_component_version(
    paths: &Paths,
    global: &GlobalConfig,
    provider: Provider,
    installed_name: &str,
) -> anyhow::Result<String> {
    let tag = pinned_tag(provider, installed_name)?;
    let cancellation = Cancellation::new()?;
    let client = Client::new()?;
    install_selected(
        paths,
        global,
        Selection {
            provider,
            tag: Some(&tag),
            expected_name: Some(installed_name),
        },
        &client,
        &cancellation,
        &mut io::stderr(),
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
    let release = cancellation.result(client.select(selection.provider, selection.tag))?;
    if let Some(installed_name) = selection.expected_name {
        anyhow::ensure!(
            release.installed_name == installed_name,
            "release {} installs `{}`, not requested build `{installed_name}`",
            release.tag,
            release.installed_name
        );
    }
    writeln!(
        output,
        "{}",
        describe(paths, global, &release, style::stderr_enabled())?
    )?;
    install(paths, global, client, &release, cancellation)
}

fn pinned_tag(provider: Provider, installed_name: &str) -> anyhow::Result<String> {
    let prefix = match provider {
        Provider::Dxvk => "dxvk-",
        Provider::Vkd3d => "vkd3d-proton-",
        _ => anyhow::bail!("only components can use a pinned build download"),
    };
    let version = installed_name
        .strip_prefix(prefix)
        .filter(|v| !v.is_empty());
    anyhow::ensure!(
        version.is_some(),
        "`{installed_name}` is not an upstream {provider} build name"
    );
    Ok(format!("v{}", version.unwrap()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::io::{Cursor, Read};
    use std::path::Path;

    use super::*;
    use crate::prompt::Script;

    struct FixtureTransport(BTreeMap<String, String>);

    impl Transport for FixtureTransport {
        fn get(&self, url: &str) -> anyhow::Result<Box<dyn Read>> {
            let data = self
                .0
                .get(url)
                .ok_or_else(|| anyhow::anyhow!("no fixture for {url}"))?;
            Ok(Box::new(Cursor::new(data.clone().into_bytes())))
        }
    }

    fn fixture(pages: &[Vec<&str>]) -> Client<FixtureTransport> {
        let mut responses = BTreeMap::new();
        for (index, releases) in pages.iter().enumerate() {
            let rows: Vec<String> = releases
                .iter()
                .map(|tag| {
                    let version = tag.trim_start_matches('v');
                    format!(
                        r#"{{"tag_name":"{tag}","assets":[{{"name":"dxvk-{version}.tar.gz","browser_download_url":"https://fixtures.invalid/{version}","size":1048576,"digest":"sha256:{}"}}]}}"#,
                        "0".repeat(64)
                    )
                })
                .collect();
            responses.insert(
                format!(
                    "https://api.github.com/repos/doitsujin/dxvk/releases?per_page=30&page={}",
                    index + 1
                ),
                format!("[{}]", rows.join(",")),
            );
        }
        Client::with_transport(FixtureTransport(responses))
    }

    fn paths(home: &Path) -> Paths {
        Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(home))).unwrap()
    }

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

    #[test]
    fn picker_shows_three_latest_and_details_once() {
        let root = tempfile::tempdir().unwrap();
        let client = fixture(&[vec!["v2.6", "v2.5", "v2.4", "v2.3"]]);
        let mut prompt = Prompt::new(Script::new(&["1", "n", "b"]), Vec::new());
        let cancellation = Cancellation::new().unwrap();
        let result = pick_release(
            &paths(root.path()),
            &GlobalConfig::default(),
            &mut prompt,
            Provider::Dxvk,
            &client,
            &cancellation,
            false,
        )
        .unwrap();
        assert!(matches!(result, Reply::Back));
        let output = String::from_utf8(prompt.output().clone()).unwrap();
        assert!(output.contains("v2.6  1 MiB  [latest]"), "{output}");
        assert!(output.contains("v2.4  1 MiB"), "{output}");
        assert!(!output.contains("v2.3"), "{output}");
        assert!(output.contains("m) more"), "{output}");
        assert_eq!(output.matches("asset: dxvk-2.6.tar.gz").count(), 1);
        assert_eq!(
            output.matches("verification: GitHub asset SHA-256").count(),
            1
        );
        assert_eq!(output.matches("destination:").count(), 1);
    }

    #[test]
    fn more_paginates_and_can_return_to_short_list() {
        let root = tempfile::tempdir().unwrap();
        let first: Vec<String> = (1..=30).rev().map(|n| format!("v2.{n}")).collect();
        let first: Vec<&str> = first.iter().map(String::as_str).collect();
        let client = fixture(&[first, vec!["v1.9", "v1.8"]]);
        let mut prompt = Prompt::new(Script::new(&["m", "n", "p", "b", "b"]), Vec::new());
        let cancellation = Cancellation::new().unwrap();
        assert!(matches!(
            pick_release(
                &paths(root.path()),
                &GlobalConfig::default(),
                &mut prompt,
                Provider::Dxvk,
                &client,
                &cancellation,
                false,
            )
            .unwrap(),
            Reply::Back
        ));
        let output = String::from_utf8(prompt.output().clone()).unwrap();
        assert!(output.contains("releases (page 1)"), "{output}");
        assert!(output.contains("releases (page 2)"), "{output}");
        assert!(output.contains("v1.8"), "{output}");
        assert!(output.contains("Next page"), "{output}");
        assert!(output.contains("Previous page"), "{output}");
    }

    #[test]
    fn short_list_handles_fewer_than_three_releases() {
        let root = tempfile::tempdir().unwrap();
        let client = fixture(&[vec!["v2.6", "v2.5"]]);
        let mut prompt = Prompt::new(Script::new(&["b"]), Vec::new());
        let cancellation = Cancellation::new().unwrap();
        pick_release(
            &paths(root.path()),
            &GlobalConfig::default(),
            &mut prompt,
            Provider::Dxvk,
            &client,
            &cancellation,
            false,
        )
        .unwrap();
        let output = String::from_utf8(prompt.output().clone()).unwrap();
        assert!(output.contains("v2.6"));
        assert!(output.contains("v2.5"));
        assert!(!output.contains("3)"));
    }

    #[test]
    fn list_prints_every_release_and_next_page_hint() {
        let first: Vec<String> = (1..=30).rev().map(|n| format!("v2.{n}")).collect();
        let first: Vec<&str> = first.iter().map(String::as_str).collect();
        let client = fixture(&[first, vec!["v1.9", "v1.8"]]);
        let cancellation = Cancellation::new().unwrap();
        let mut output = Vec::new();
        let mut notices = Vec::new();
        list_to(
            &client,
            Provider::Dxvk,
            1,
            false,
            &cancellation,
            &mut output,
            &mut notices,
        )
        .unwrap();
        assert_eq!(String::from_utf8(output).unwrap().lines().count(), 30);
        assert!(String::from_utf8(notices).unwrap().contains("--page 2"));
        let mut output = Vec::new();
        let mut notices = Vec::new();
        list_to(
            &client,
            Provider::Dxvk,
            2,
            true,
            &cancellation,
            &mut output,
            &mut notices,
        )
        .unwrap();
        let output = String::from_utf8(output).unwrap();
        assert_eq!(output.lines().count(), 2);
        assert!(output.contains("dxvk-1.9"), "{output}");
        assert!(notices.is_empty());
    }

    #[test]
    fn quick_selection_prints_details_once_before_transfer() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let client = fixture(&[vec!["v2.6"]]);
        let cancellation = Cancellation::new().unwrap();
        let mut output = Vec::new();
        let result = install_selected(
            &paths,
            &GlobalConfig::default(),
            Selection {
                provider: Provider::Dxvk,
                tag: None,
                expected_name: None,
            },
            &client,
            &cancellation,
            &mut output,
        );
        assert!(result.is_err());
        let output = String::from_utf8(output).unwrap();
        assert_eq!(output.matches("asset: dxvk-2.6.tar.gz").count(), 1);
        assert_eq!(
            output.matches("verification: GitHub asset SHA-256").count(),
            1
        );
        assert_eq!(output.matches("destination:").count(), 1);
    }

    #[test]
    fn pinned_component_names_select_exact_tags() {
        assert_eq!(pinned_tag(Provider::Dxvk, "dxvk-2.6").unwrap(), "v2.6");
        assert_eq!(
            pinned_tag(Provider::Vkd3d, "vkd3d-proton-2.14").unwrap(),
            "v2.14"
        );
        assert!(pinned_tag(Provider::Dxvk, "custom-build").is_err());
        assert!(pinned_tag(Provider::Dxvk, "2.7").is_err());
        assert!(pinned_tag(Provider::GeProton, "dxvk-2.6").is_err());
    }
}
