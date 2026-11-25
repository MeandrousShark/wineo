use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::rc::Rc;

use sha2::{Digest, Sha256};

use super::*;
use crate::test_script::write_executable;

enum Reply {
    Body(Vec<u8>),
    RateLimited,
}

/// A fixture transport that remembers every address asked for, and can
/// cancel a token when one of them is.
#[derive(Default)]
struct Fake {
    replies: BTreeMap<String, Reply>,
    asked: Rc<RefCell<Vec<String>>>,
    cancel_on: Option<(String, Token)>,
}

impl Transport for Fake {
    fn get(&self, url: &str) -> Result<Box<dyn Read>> {
        self.asked.borrow_mut().push(url.to_owned());
        if let Some((at, token)) = &self.cancel_on
            && at == url
        {
            token.cancel();
        }
        match self.replies.get(url) {
            Some(Reply::Body(body)) => Ok(Box::new(Cursor::new(body.clone()))),
            Some(Reply::RateLimited) => {
                Err(RateLimited::status(reqwest::StatusCode::FORBIDDEN).to_error())
            }
            None => bail!("unexpected fixture URL {url}"),
        }
    }
}

impl Fake {
    fn reply(&mut self, url: impl Into<String>, reply: Reply) {
        self.replies.insert(url.into(), reply);
    }

    /// The newest release of `provider` is `tag`, shipping `asset`.
    fn newest(&mut self, provider: Provider, tag: &str, asset: &str) {
        let digest = format!("sha256:{}", "0".repeat(64));
        self.catalog(provider, tag, asset, 1, Some(&digest));
    }

    fn catalog(
        &mut self,
        provider: Provider,
        tag: &str,
        asset: &str,
        size: u64,
        digest: Option<&str>,
    ) {
        let body = serde_json::json!([{
            "tag_name": tag,
            "assets": [{
                "name": asset,
                "browser_download_url": asset_url(asset),
                "size": size,
                "digest": digest,
            }],
        }]);
        self.reply(
            catalog_url(provider),
            Reply::Body(serde_json::to_vec(&body).unwrap()),
        );
    }
}

fn asset_url(asset: &str) -> String {
    format!("https://fixtures/{asset}")
}

fn catalog_url(provider: Provider) -> String {
    let repo = match provider {
        Provider::GeProton => "GloriousEggroll/proton-ge-custom",
        Provider::ProtonCachyos => "CachyOS/proton-cachyos",
        Provider::Dxvk => "doitsujin/dxvk",
        Provider::Vkd3d => "HansKristian-Work/vkd3d-proton",
    };
    format!("https://api.github.com/repos/{repo}/releases?per_page=30&page=1")
}

struct Home {
    _dir: tempfile::TempDir,
    paths: Paths,
    global: GlobalConfig,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().as_os_str().to_owned();
        let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.clone())).unwrap();
        Self {
            _dir: dir,
            paths,
            global: GlobalConfig::default(),
        }
    }

    fn root(&self, provider: Provider) -> PathBuf {
        provider.root(&self.paths, &self.global).unwrap()
    }

    /// A Proton build of stub executables, enough for the alias's rule.
    fn runner(&self, provider: Provider, name: &str) -> PathBuf {
        let dir = self.root(provider).join(name);
        proton(&dir);
        dir
    }

    /// A component build of stub DLLs, both architectures.
    fn component(&self, provider: Provider, name: &str) -> PathBuf {
        let dir = self.root(provider).join(name);
        let (narrow, marker) = if provider == Provider::Dxvk {
            ("x32", "dxgi.dll")
        } else {
            ("x86", "d3d12.dll")
        };
        for arch in ["x64", narrow] {
            fs::create_dir_all(dir.join(arch)).unwrap();
            fs::write(dir.join(arch).join(marker), "").unwrap();
        }
        dir
    }

    fn check(&self, fake: Fake) -> Report {
        check(
            &Client::with_transport(fake),
            &self.paths,
            &self.global,
            &[],
            &Token::new(),
        )
    }
}

fn proton(dir: &Path) {
    for (file, mode) in [
        ("proton", 0o755),
        ("files/bin/wine", 0o755),
        ("files/bin/wineserver", 0o755),
    ] {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_executable(&path, "#!/bin/sh\n", mode);
    }
}

/// A download's receipt. A runner's version is its tag, which the alias's
/// rule reads in place of the folder name.
fn receipt(dir: &Path, provider: &str, version: &str, stable: bool) {
    let receipt = serde_json::json!({
        "provider": provider,
        "version": version,
        "architecture": "x86_64",
        "variant": "baseline",
        "stable": stable,
        "asset": "a.tar.gz",
        "digest": "",
    });
    fs::write(dir.join(RECEIPT), receipt.to_string()).unwrap();
}

/// Each family as one line: its installed build and what the check said.
fn summary(report: &Report) -> Vec<String> {
    report
        .families
        .iter()
        .map(|family| {
            let status = match &family.status {
                Status::UpToDate => "up to date".to_owned(),
                Status::Available(release) => format!("-> {}", release.installed_name),
                Status::Failed(error) => format!("failed: {error}"),
            };
            format!(
                "{} {} {status}",
                family.provider,
                family.installed.as_deref().unwrap_or("?")
            )
        })
        .collect()
}

#[test]
fn each_family_alone_is_checked_against_its_own_newest_release() {
    let cases = [
        (
            Provider::GeProton,
            "GE-Proton10-20",
            "GE-Proton10-27",
            "GE-Proton10-27.tar.gz",
            "GE-Proton10-27",
        ),
        (
            Provider::ProtonCachyos,
            "proton-cachyos-10.0-20261001-slr-x86_64",
            "cachyos-10.0-20261101-slr",
            "proton-cachyos-10.0-20261101-slr-x86_64.tar.xz",
            "proton-cachyos-10.0-20261101-slr-x86_64",
        ),
        (
            Provider::Dxvk,
            "dxvk-2.6",
            "v2.7.1",
            "dxvk-2.7.1.tar.gz",
            "dxvk-2.7.1",
        ),
        (
            Provider::Vkd3d,
            "vkd3d-proton-2.13",
            "v2.14",
            "vkd3d-proton-2.14.tar.zst",
            "vkd3d-proton-2.14",
        ),
    ];
    for (provider, installed, tag, asset, newer) in cases {
        let home = Home::new();
        if matches!(provider, Provider::GeProton | Provider::ProtonCachyos) {
            home.runner(provider, installed);
        } else {
            home.component(provider, installed);
        }
        let mut fake = Fake::default();
        fake.newest(provider, tag, asset);
        let asked = fake.asked.clone();
        // Nothing else is installed, so nothing else is asked about.
        assert_eq!(
            summary(&home.check(fake)),
            [format!("{provider} {installed} -> {newer}")]
        );
        assert_eq!(*asked.borrow(), [catalog_url(provider)]);
    }
}

#[test]
fn nothing_installed_asks_nothing() {
    let home = Home::new();
    let fake = Fake::default();
    let asked = fake.asked.clone();
    assert!(home.check(fake).families.is_empty());
    assert!(asked.borrow().is_empty());
}

#[test]
fn a_receipt_from_another_provider_doesnt_count() {
    let home = Home::new();
    let ge = home.runner(Provider::GeProton, "GE-Proton10-20");
    receipt(&ge, "proton-cachyos", "GE-Proton10-20", true);
    let dxvk = home.component(Provider::Dxvk, "dxvk-2.6");
    receipt(&dxvk, "vkd3d", "v2.6", true);
    assert!(home.check(Fake::default()).families.is_empty());

    // The same builds with their own provider's receipt do count.
    receipt(&ge, "ge-proton", "GE-Proton10-20", true);
    receipt(&dxvk, "dxvk", "v2.6", true);
    let mut fake = Fake::default();
    fake.newest(
        Provider::GeProton,
        "GE-Proton10-20",
        "GE-Proton10-20.tar.gz",
    );
    fake.newest(Provider::Dxvk, "v2.6", "dxvk-2.6.tar.gz");
    assert_eq!(
        summary(&home.check(fake)),
        [
            "ge-proton GE-Proton10-20 up to date",
            "dxvk dxvk-2.6 up to date"
        ]
    );
}

#[test]
fn a_component_receipt_must_be_a_stable_download() {
    let home = Home::new();
    let dxvk = home.component(Provider::Dxvk, "dxvk-2.6");
    receipt(&dxvk, "dxvk", "v2.6", false);
    assert!(home.check(Fake::default()).families.is_empty());
    fs::write(dxvk.join(RECEIPT), "broken").unwrap();
    assert!(home.check(Fake::default()).families.is_empty());
}

#[test]
fn a_build_without_a_receipt_goes_by_its_folder_name() {
    let home = Home::new();
    home.runner(Provider::GeProton, "GE-Proton10-20");
    home.component(Provider::Dxvk, "dxvk-2.6");
    let mut fake = Fake::default();
    fake.newest(
        Provider::GeProton,
        "GE-Proton10-27",
        "GE-Proton10-27.tar.gz",
    );
    fake.newest(Provider::Dxvk, "v2.7", "dxvk-2.7.tar.gz");
    assert_eq!(
        summary(&home.check(fake)),
        [
            "ge-proton GE-Proton10-20 -> GE-Proton10-27",
            "dxvk dxvk-2.6 -> dxvk-2.7"
        ]
    );
}

#[test]
fn a_half_extracted_build_with_the_right_name_doesnt_count() {
    let home = Home::new();
    let ge = home.runner(Provider::GeProton, "GE-Proton10-27");
    fs::remove_file(ge.join("files/bin/wineserver")).unwrap();
    let dxvk = home.component(Provider::Dxvk, "dxvk-2.7");
    fs::remove_dir_all(dxvk.join("x32")).unwrap();
    assert!(home.check(Fake::default()).families.is_empty());
    // The alias agrees, since both go by the same rule.
    assert!(
        Runners::at(home.root(Provider::GeProton))
            .resolve("GE-Proton-Latest")
            .is_err()
    );
}

#[test]
fn a_component_build_named_unlike_a_download_doesnt_count() {
    let home = Home::new();
    home.component(Provider::Dxvk, "dxvk-2.6");
    for name in ["dxvk-gplasync-2.8", "dxvk-2.9-x86_64", "2.9", "dxvk-v2.9"] {
        home.component(Provider::Dxvk, name);
    }
    let mut fake = Fake::default();
    fake.newest(Provider::Dxvk, "v2.7", "dxvk-2.7.tar.gz");
    assert_eq!(summary(&home.check(fake)), ["dxvk dxvk-2.6 -> dxvk-2.7"]);
}

#[test]
fn vkd3d_proton_compares_folder_names_not_tags() {
    let home = Home::new();
    home.component(Provider::Vkd3d, "vkd3d-proton-2.14");
    let mut fake = Fake::default();
    fake.newest(Provider::Vkd3d, "v2.14", "vkd3d-proton-2.14.tar.zst");
    assert_eq!(
        summary(&home.check(fake)),
        ["vkd3d vkd3d-proton-2.14 up to date"]
    );

    let home = Home::new();
    home.component(Provider::Vkd3d, "vkd3d-proton-2.13");
    let mut fake = Fake::default();
    fake.newest(Provider::Vkd3d, "v2.14", "vkd3d-proton-2.14.tar.zst");
    assert_eq!(
        summary(&home.check(fake)),
        ["vkd3d vkd3d-proton-2.13 -> vkd3d-proton-2.14"]
    );
}

#[test]
fn another_launchers_build_is_ignored() {
    let mut home = Home::new();
    home.global.external_runners = Some(true);
    proton(
        &home
            .paths
            .home()
            .join(".local/share/Steam/compatibilitytools.d/GE-Proton10-20"),
    );
    // The runner list does see it, so it is a real external build.
    let listing = Runners::new(&home.paths, &home.global)
        .unwrap()
        .list()
        .unwrap();
    assert_eq!(listing.runners.len(), 1);
    assert!(home.check(Fake::default()).families.is_empty());
}

#[test]
fn an_older_upstream_release_is_up_to_date() {
    let home = Home::new();
    home.runner(Provider::GeProton, "GE-Proton10-27");
    home.component(Provider::Dxvk, "dxvk-2.7");
    let mut fake = Fake::default();
    fake.newest(
        Provider::GeProton,
        "GE-Proton10-20",
        "GE-Proton10-20.tar.gz",
    );
    fake.newest(Provider::Dxvk, "v2.6", "dxvk-2.6.tar.gz");
    assert_eq!(
        summary(&home.check(fake)),
        [
            "ge-proton GE-Proton10-27 up to date",
            "dxvk dxvk-2.7 up to date"
        ]
    );
}

#[test]
fn the_newest_installed_build_is_the_one_compared() {
    let home = Home::new();
    for name in ["GE-Proton9-20", "GE-Proton10-27", "GE-Proton10-3"] {
        home.runner(Provider::GeProton, name);
    }
    let mut fake = Fake::default();
    fake.newest(
        Provider::GeProton,
        "GE-Proton10-20",
        "GE-Proton10-20.tar.gz",
    );
    assert_eq!(
        summary(&home.check(fake)),
        ["ge-proton GE-Proton10-27 up to date"]
    );
}

#[test]
fn a_tag_that_wont_parse_is_that_familys_error() {
    let home = Home::new();
    home.runner(Provider::GeProton, "GE-Proton10-20");
    let mut fake = Fake::default();
    fake.newest(Provider::GeProton, "GE-Proton11", "GE-Proton11.tar.gz");
    assert_eq!(
        summary(&home.check(fake)),
        ["ge-proton GE-Proton10-20 failed: can't compare GE-Proton's newest release, GE-Proton11"]
    );
}

#[test]
fn a_component_release_whose_folder_name_fails_the_rule_is_that_familys_error() {
    let home = Home::new();
    home.component(Provider::Dxvk, "dxvk-2.4");
    let mut fake = Fake::default();
    fake.newest(Provider::Dxvk, "v2.5-1", "dxvk-2.5-1.tar.gz");
    assert_eq!(
        summary(&home.check(fake)),
        ["dxvk dxvk-2.4 failed: can't compare DXVK's newest release, v2.5-1"]
    );
}

#[test]
fn a_newer_release_with_no_checksums_is_that_familys_error() {
    let home = Home::new();
    home.component(Provider::Dxvk, "dxvk-2.6");
    let mut fake = Fake::default();
    fake.catalog(Provider::Dxvk, "v2.7", "dxvk-2.7.tar.gz", 1, None);
    let asked = fake.asked.clone();
    let report = home.check(fake);
    assert_eq!(
        summary(&report),
        [
            "dxvk dxvk-2.6 failed: dxvk-2.7.tar.gz has no supported upstream checksum or GitHub SHA-256 digest; refusing unchecked installation"
        ]
    );
    // Read from the release metadata alone.
    assert_eq!(*asked.borrow(), [catalog_url(Provider::Dxvk)]);
    assert!(report.failed());
    assert_eq!(report.updates().count(), 0);
}

#[test]
fn the_rate_limit_stops_the_rest_without_asking_again() {
    let home = Home::new();
    home.runner(Provider::GeProton, "GE-Proton10-20");
    home.component(Provider::Dxvk, "dxvk-2.6");
    home.component(Provider::Vkd3d, "vkd3d-proton-2.13");
    let mut fake = Fake::default();
    fake.reply(catalog_url(Provider::GeProton), Reply::RateLimited);
    fake.newest(Provider::Dxvk, "v2.7", "dxvk-2.7.tar.gz");
    fake.newest(Provider::Vkd3d, "v2.14", "vkd3d-proton-2.14.tar.zst");
    let asked = fake.asked.clone();
    let report = home.check(fake);
    let limited =
        "failed: GitHub denied or rate-limited the request (403 Forbidden); try again later";
    assert_eq!(
        summary(&report),
        [
            format!("ge-proton GE-Proton10-20 {limited}"),
            format!("dxvk dxvk-2.6 {limited}"),
            format!("vkd3d vkd3d-proton-2.13 {limited}"),
        ]
    );
    assert_eq!(*asked.borrow(), [catalog_url(Provider::GeProton)]);
    for family in &report.families {
        let Status::Failed(error) = &family.status else {
            panic!("{family:?}");
        };
        assert!(download::rate_limited(error).is_some(), "{error}");
    }
}

#[test]
fn one_familys_failed_check_leaves_the_others_running() {
    let home = Home::new();
    home.runner(Provider::GeProton, "GE-Proton10-20");
    home.component(Provider::Dxvk, "dxvk-2.6");
    // No catalog for GE-Proton, so asking for it fails.
    let mut fake = Fake::default();
    fake.newest(Provider::Dxvk, "v2.7", "dxvk-2.7.tar.gz");
    let report = home.check(fake);
    assert_eq!(
        summary(&report),
        [
            format!(
                "ge-proton GE-Proton10-20 failed: unexpected fixture URL {}",
                catalog_url(Provider::GeProton)
            ),
            "dxvk dxvk-2.6 -> dxvk-2.7".to_owned(),
        ]
    );
    // An ordinary failure isn't the rate limit.
    let Status::Failed(error) = &report.families[0].status else {
        unreachable!()
    };
    assert!(download::rate_limited(error).is_none());
}

#[test]
fn a_cancel_between_two_checks_skips_the_rest() {
    let home = Home::new();
    home.runner(Provider::GeProton, "GE-Proton10-20");
    home.component(Provider::Dxvk, "dxvk-2.6");
    let token = Token::new();
    let mut fake = Fake {
        cancel_on: Some((catalog_url(Provider::GeProton), token.clone())),
        ..Fake::default()
    };
    fake.newest(
        Provider::GeProton,
        "GE-Proton10-27",
        "GE-Proton10-27.tar.gz",
    );
    fake.newest(Provider::Dxvk, "v2.7", "dxvk-2.7.tar.gz");
    let asked = fake.asked.clone();
    let report = check(
        &Client::with_transport(fake),
        &home.paths,
        &home.global,
        &[],
        &token,
    );
    // The answer already asked for is kept; the next family is never asked.
    assert_eq!(
        summary(&report),
        [
            "ge-proton GE-Proton10-20 -> GE-Proton10-27",
            "dxvk dxvk-2.6 failed: update check canceled"
        ]
    );
    assert_eq!(*asked.borrow(), [catalog_url(Provider::GeProton)]);
}

fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, path, *bytes).unwrap();
    }
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gzip.write_all(&builder.into_inner().unwrap()).unwrap();
    gzip.finish().unwrap()
}

/// Installed DXVK 2.6 and vkd3d-proton 2.13, with 2.7 and 2.14 upstream and
/// their archives ready to download.
fn two_updates(home: &Home, fake: &mut Fake) {
    home.component(Provider::Dxvk, "dxvk-2.6");
    home.component(Provider::Vkd3d, "vkd3d-proton-2.13");
    for (provider, tag, asset, entries) in [
        (
            Provider::Dxvk,
            "v2.7",
            "dxvk-2.7.tar.gz",
            [
                ("dxvk-2.7/x64/dxgi.dll", &b"wide"[..]),
                ("dxvk-2.7/x32/dxgi.dll", &b"narrow"[..]),
            ],
        ),
        (
            Provider::Vkd3d,
            "v2.14",
            "vkd3d-proton-2.14.tar.gz",
            [
                ("vkd3d-proton-2.14/x64/d3d12.dll", &b"wide"[..]),
                ("vkd3d-proton-2.14/x86/d3d12.dll", &b"narrow"[..]),
            ],
        ),
    ] {
        let bytes = archive(&entries);
        let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
        fake.catalog(provider, tag, asset, bytes.len() as u64, Some(&digest));
        fake.reply(asset_url(asset), Reply::Body(bytes));
    }
}

fn outcomes(applied: &Applied) -> Vec<String> {
    applied
        .families
        .iter()
        .map(|(provider, outcome)| match outcome {
            Outcome::Installed(path) => format!(
                "{provider} installed {}",
                path.file_name().unwrap().display()
            ),
            Outcome::AlreadyThere(path) => {
                format!("{provider} already {}", path.file_name().unwrap().display())
            }
            Outcome::Failed(error) => format!("{provider} failed: {error}"),
        })
        .collect()
}

/// Checks, then applies the report with the same client and token.
fn check_and_apply(home: &Home, fake: Fake, token: &Token) -> (Report, Applied, Vec<String>) {
    let client = Client::with_transport(fake);
    let report = check(&client, &home.paths, &home.global, &[], &Token::new());
    let mut steps = Vec::new();
    let applied = apply(&client, &home.paths, &home.global, &report, token, |step| {
        if !matches!(step.progress, Progress::Downloading { .. }) {
            steps.push(format!(
                "{} of {} {} {:?}",
                step.index + 1,
                step.count,
                step.release.installed_name,
                step.progress
            ));
        }
    });
    (report, applied, steps)
}

#[test]
fn apply_installs_each_update_then_finds_them_already_there() {
    let home = Home::new();
    let mut fake = Fake::default();
    two_updates(&home, &mut fake);
    let client = Client::with_transport(fake);
    let report = check(&client, &home.paths, &home.global, &[], &Token::new());
    let mut steps = Vec::new();
    let applied = apply(
        &client,
        &home.paths,
        &home.global,
        &report,
        &Token::new(),
        |step| steps.push((step.index, step.count, step.release.provider)),
    );
    assert_eq!(
        outcomes(&applied),
        [
            "dxvk installed dxvk-2.7",
            "vkd3d installed vkd3d-proton-2.14"
        ]
    );
    assert!(!applied.failed());
    assert!(steps.contains(&(0, 2, Provider::Dxvk)));
    assert!(steps.contains(&(1, 2, Provider::Vkd3d)));
    // The older builds stay beside the new ones.
    assert!(home.root(Provider::Dxvk).join("dxvk-2.6").is_dir());

    let again = apply(
        &client,
        &home.paths,
        &home.global,
        &report,
        &Token::new(),
        |_| {},
    );
    assert_eq!(
        outcomes(&again),
        ["dxvk already dxvk-2.7", "vkd3d already vkd3d-proton-2.14"]
    );
    // And a fresh check now reads both as up to date.
    let checked = check(&client, &home.paths, &home.global, &[], &Token::new());
    assert_eq!(
        summary(&checked),
        [
            "dxvk dxvk-2.7 up to date",
            "vkd3d vkd3d-proton-2.14 up to date"
        ]
    );
}

#[test]
fn one_familys_failed_download_leaves_the_others_running() {
    let home = Home::new();
    let mut fake = Fake::default();
    two_updates(&home, &mut fake);
    fake.reply(
        asset_url("dxvk-2.7.tar.gz"),
        Reply::Body(b"corrupt".to_vec()),
    );
    let (_, applied, _) = check_and_apply(&home, fake, &Token::new());
    assert_eq!(
        outcomes(&applied),
        [
            format!(
                "dxvk failed: incomplete archive: expected {} bytes, received 7",
                dxvk_size()
            ),
            "vkd3d installed vkd3d-proton-2.14".to_owned(),
        ]
    );
    assert!(applied.failed());
    assert!(!home.root(Provider::Dxvk).join("dxvk-2.7").exists());
}

fn dxvk_size() -> String {
    archive(&[
        ("dxvk-2.7/x64/dxgi.dll", &b"wide"[..]),
        ("dxvk-2.7/x32/dxgi.dll", &b"narrow"[..]),
    ])
    .len()
    .to_string()
}

#[test]
fn the_rate_limit_stops_the_rest_of_the_downloads() {
    let home = Home::new();
    let mut fake = Fake::default();
    two_updates(&home, &mut fake);
    fake.reply(asset_url("dxvk-2.7.tar.gz"), Reply::RateLimited);
    let asked = fake.asked.clone();
    let (_, applied, _) = check_and_apply(&home, fake, &Token::new());
    let limited =
        "failed: GitHub denied or rate-limited the request (403 Forbidden); try again later";
    assert_eq!(
        outcomes(&applied),
        [format!("dxvk {limited}"), format!("vkd3d {limited}")]
    );
    assert!(
        !asked
            .borrow()
            .contains(&asset_url("vkd3d-proton-2.14.tar.gz"))
    );
}

#[test]
fn a_cancel_between_two_downloads_keeps_the_first_and_skips_the_rest() {
    let home = Home::new();
    let token = Token::new();
    let mut fake = Fake::default();
    two_updates(&home, &mut fake);
    let asked = fake.asked.clone();
    let client = Client::with_transport(fake);
    let report = check(&client, &home.paths, &home.global, &[], &Token::new());
    let applied = apply(
        &client,
        &home.paths,
        &home.global,
        &report,
        &token,
        |step| {
            if matches!(step.progress, Progress::Installed(_)) {
                token.cancel();
            }
        },
    );
    assert_eq!(
        outcomes(&applied),
        ["dxvk installed dxvk-2.7", "vkd3d failed: download canceled"]
    );
    assert!(home.root(Provider::Dxvk).join("dxvk-2.7").is_dir());
    assert!(
        !asked
            .borrow()
            .contains(&asset_url("vkd3d-proton-2.14.tar.gz"))
    );
}

#[test]
fn apply_reports_each_step_in_batch_order() {
    let home = Home::new();
    let mut fake = Fake::default();
    two_updates(&home, &mut fake);
    let (report, _, steps) = check_and_apply(&home, fake, &Token::new());
    assert_eq!(report.updates().count(), 2);
    let first = steps
        .iter()
        .position(|s| s.starts_with("1 of 2 dxvk-2.7 Installed"));
    let second = steps
        .iter()
        .position(|s| s.starts_with("2 of 2 vkd3d-proton-2.14 Installed"));
    assert!(
        first.is_some() && second.is_some() && first < second,
        "{steps:?}"
    );
}

fn check_named(home: &Home, fake: Fake, wanted: &[Provider]) -> Report {
    check(
        &Client::with_transport(fake),
        &home.paths,
        &home.global,
        wanted,
        &Token::new(),
    )
}

/// GE-Proton and DXVK installed, each with a newer release upstream.
fn two_installed(home: &Home) -> Fake {
    home.runner(Provider::GeProton, "GE-Proton10-20");
    home.component(Provider::Dxvk, "dxvk-2.6");
    let mut fake = Fake::default();
    fake.newest(
        Provider::GeProton,
        "GE-Proton10-27",
        "GE-Proton10-27.tar.gz",
    );
    fake.newest(Provider::Dxvk, "v2.7", "dxvk-2.7.tar.gz");
    fake
}

#[test]
fn a_named_family_is_the_only_one_asked_about() {
    let home = Home::new();
    let fake = two_installed(&home);
    let asked = fake.asked.clone();
    let report = check_named(&home, fake, &[Provider::Dxvk]);
    assert_eq!(summary(&report), ["dxvk dxvk-2.6 -> dxvk-2.7"]);
    assert_eq!(*asked.borrow(), [catalog_url(Provider::Dxvk)]);
}

#[test]
fn a_family_named_twice_counts_once() {
    let home = Home::new();
    let fake = two_installed(&home);
    let asked = fake.asked.clone();
    let report = check_named(
        &home,
        fake,
        &[Provider::Dxvk, Provider::GeProton, Provider::Dxvk],
    );
    // In the providers' order, not the order named.
    assert_eq!(
        summary(&report),
        [
            "ge-proton GE-Proton10-20 -> GE-Proton10-27",
            "dxvk dxvk-2.6 -> dxvk-2.7"
        ]
    );
    assert_eq!(
        *asked.borrow(),
        [catalog_url(Provider::GeProton), catalog_url(Provider::Dxvk)]
    );
}

#[test]
fn a_named_family_that_isnt_installed_is_its_error() {
    let home = Home::new();
    let fake = two_installed(&home);
    let asked = fake.asked.clone();
    let report = check_named(&home, fake, &[Provider::ProtonCachyos, Provider::Dxvk]);
    assert_eq!(
        summary(&report),
        [
            "proton-cachyos ? failed: Proton-CachyOS isn't installed",
            "dxvk dxvk-2.6 -> dxvk-2.7"
        ]
    );
    let Status::Failed(error) = &report.families[0].status else {
        unreachable!()
    };
    assert_eq!(
        error.downcast_ref::<NotInstalled>(),
        Some(&NotInstalled(Provider::ProtonCachyos))
    );
    assert_eq!(*asked.borrow(), [catalog_url(Provider::Dxvk)]);
}

#[test]
fn no_family_named_means_every_installed_one() {
    let home = Home::new();
    let fake = two_installed(&home);
    let asked = fake.asked.clone();
    let report = check_named(&home, fake, &[]);
    // Proton-CachyOS and vkd3d-proton aren't installed, so they're left out.
    assert_eq!(
        summary(&report),
        [
            "ge-proton GE-Proton10-20 -> GE-Proton10-27",
            "dxvk dxvk-2.6 -> dxvk-2.7"
        ]
    );
    assert_eq!(asked.borrow().len(), 2);
}

#[test]
fn a_family_whose_root_cant_be_read_fails_and_the_rest_still_report() {
    let home = Home::new();
    let root = home.root(Provider::GeProton);
    fs::create_dir_all(root.parent().unwrap()).unwrap();
    fs::write(&root, "not a folder").unwrap();
    home.component(Provider::Dxvk, "dxvk-2.6");
    let mut fake = Fake::default();
    fake.newest(Provider::Dxvk, "v2.7", "dxvk-2.7.tar.gz");
    let report = home.check(fake);
    // Both runner families share that root, so both fail.
    assert_eq!(report.families.len(), 3, "{report:?}");
    for (family, provider) in report
        .families
        .iter()
        .zip([Provider::GeProton, Provider::ProtonCachyos])
    {
        assert_eq!(family.provider, provider);
        assert!(family.installed.is_none(), "{family:?}");
        assert!(matches!(family.status, Status::Failed(_)), "{family:?}");
    }
    assert_eq!(summary(&report)[2], "dxvk dxvk-2.6 -> dxvk-2.7");
}

#[test]
fn a_leading_zero_reads_as_the_same_version() {
    let home = Home::new();
    // `latest` reads names the same way, by the value of each digit run.
    home.component(Provider::Dxvk, "dxvk-2.07");
    let mut fake = Fake::default();
    fake.newest(Provider::Dxvk, "v2.7", "dxvk-2.7.tar.gz");
    assert_eq!(summary(&home.check(fake)), ["dxvk dxvk-2.07 up to date"]);
}

#[test]
fn older_builds_are_the_family_below_its_newest() {
    let home = Home::new();
    for name in ["GE-Proton10-27", "GE-Proton10-18", "GE-Proton10-20"] {
        home.runner(Provider::GeProton, name);
    }
    // Another family, and a name that isn't GE-Proton's, aren't its builds.
    home.runner(Provider::ProtonCachyos, "proton-cachyos-10.0-20261001-slr");
    home.runner(Provider::GeProton, "GE-Proton10-custom");
    for name in ["dxvk-2.6", "dxvk-2.5", "dxvk-gplasync-2.4"] {
        home.component(Provider::Dxvk, name);
    }
    home.component(Provider::Vkd3d, "vkd3d-proton-2.13");
    let older = |provider| older_builds(&home.paths, &home.global, provider).unwrap();
    assert_eq!(
        older(Provider::GeProton),
        ["GE-Proton10-18", "GE-Proton10-20"]
    );
    assert!(older(Provider::ProtonCachyos).is_empty());
    assert_eq!(older(Provider::Dxvk), ["dxvk-2.5"]);
    assert!(older(Provider::Vkd3d).is_empty());
}

#[test]
fn older_builds_leave_out_what_remove_would_refuse() {
    let mut home = Home::new();
    let lutris = home.paths.home().join(".local/share/lutris/runners/wine");
    fs::create_dir_all(&lutris).unwrap();
    home.global.defaults.runner_root = Some(lutris.display().to_string());
    for name in ["GE-Proton10-20", "GE-Proton10-27"] {
        home.runner(Provider::GeProton, name);
    }
    // The root is another launcher's, so those builds aren't wineo's to remove.
    assert!(
        older_builds(&home.paths, &home.global, Provider::GeProton)
            .unwrap()
            .is_empty()
    );

    // A build that is a link rather than a folder is refused too.
    let target = home.component(Provider::Dxvk, "dxvk-2.5");
    home.component(Provider::Dxvk, "dxvk-2.7");
    let link = home.root(Provider::Dxvk).join("dxvk-2.6");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert_eq!(
        older_builds(&home.paths, &home.global, Provider::Dxvk).unwrap(),
        ["dxvk-2.5"]
    );
}
