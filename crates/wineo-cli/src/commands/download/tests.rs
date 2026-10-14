use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, Cursor, Read};
use std::path::Path;
use std::process::Command;

use super::*;
use crate::prompt::Script;

struct BackInput;

impl Input for BackInput {
    fn line(&mut self, _prompt: &str) -> io::Result<prompt::Line> {
        Ok(prompt::Line::Back)
    }
}

struct FixtureTransport(BTreeMap<String, Vec<u8>>);

impl Transport for FixtureTransport {
    fn get(&self, url: &str) -> anyhow::Result<Box<dyn Read>> {
        let data = self
            .0
            .get(url)
            .ok_or_else(|| anyhow::anyhow!("no fixture for {url}"))?;
        Ok(Box::new(Cursor::new(data.clone())))
    }
}

// Tiny valid DXVK archive with both required DLLs, for offline installs.
const DXVK_ARCHIVE: &[u8] = &[
    0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0xff, 0xed, 0xd4, 0x51, 0x0a, 0x80, 0x20,
    0x0c, 0x80, 0x61, 0x8f, 0xe2, 0x09, 0x4a, 0xca, 0xec, 0x3c, 0x81, 0x11, 0x41, 0x14, 0x58, 0x91,
    0xc7, 0xcf, 0xa0, 0x82, 0x7c, 0x37, 0x82, 0xfe, 0xbd, 0x88, 0x13, 0x86, 0xfb, 0x64, 0x7a, 0xa3,
    0x73, 0xeb, 0xbb, 0x3e, 0xb3, 0xc3, 0x20, 0x12, 0x85, 0x0a, 0x61, 0xb4, 0x16, 0xc7, 0x1a, 0x22,
    0x5e, 0x43, 0xea, 0x3e, 0x3b, 0xcf, 0xeb, 0xba, 0x52, 0x42, 0xaa, 0x44, 0xf7, 0x79, 0x94, 0x5d,
    0xe7, 0xa5, 0x71, 0x52, 0x3e, 0x72, 0x3f, 0xda, 0x6c, 0xbd, 0x6d, 0x7f, 0xd4, 0x2e, 0xad, 0x46,
    0x02, 0xbe, 0x2c, 0x3e, 0x30, 0xff, 0xe6, 0xfa, 0x13, 0xee, 0xf9, 0xd7, 0x15, 0xf3, 0x1f, 0x3d,
    0x55, 0x92, 0xed, 0xd8, 0x38, 0x37, 0x6d, 0x49, 0x4a, 0x53, 0x14, 0x01, 0x04, 0x10, 0x40, 0x00,
    0x01, 0x04, 0x10, 0x40, 0x00, 0x01, 0x04, 0x10, 0x40, 0x00, 0x01, 0x04, 0x10, 0x40, 0x00, 0x01,
    0x04, 0x10, 0x40, 0x00, 0x01, 0x04, 0x10, 0x40, 0x00, 0x01, 0x04, 0x10, 0x40, 0x00, 0x01, 0x04,
    0x10, 0x40, 0xe0, 0x05, 0x81, 0x1d, 0x8b, 0xec, 0xac, 0x38, 0x00, 0x28, 0x00, 0x00,
];
const DXVK_DIGEST: &str = "sha256:254415f9efe932df0dc7a1f7b2b4ffc7244efc89847f1206d56435b7a88f5b88";

fn fixture(pages: &[Vec<&str>]) -> Client<FixtureTransport> {
    let mut responses = BTreeMap::new();
    for (index, releases) in pages.iter().enumerate() {
        let rows: Vec<String> = releases
            .iter()
            .map(|tag| {
                let version = tag.trim_start_matches('v');
                let url = format!("https://fixtures.invalid/{version}");
                responses.insert(url.clone(), DXVK_ARCHIVE.to_vec());
                format!(
                    r#"{{"tag_name":"{tag}","assets":[{{"name":"dxvk-{version}.tar.gz","browser_download_url":"{url}","size":{},"digest":"{DXVK_DIGEST}"}}]}}"#,
                    DXVK_ARCHIVE.len()
                )
            })
            .collect();
        responses.insert(
            format!(
                "https://api.github.com/repos/doitsujin/dxvk/releases?per_page=30&page={}",
                index + 1
            ),
            format!("[{}]", rows.join(",")).into_bytes(),
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
    let result = pick_release(
        &paths(root.path()),
        &GlobalConfig::default(),
        &mut prompt,
        Provider::Dxvk,
        &client,
        false,
    )
    .unwrap();
    assert!(matches!(result, Reply::Back));
    let output = String::from_utf8(prompt.output().clone()).unwrap();
    assert!(output.contains("v2.6  174 B  [latest]"), "{output}");
    assert!(output.contains("v2.4  174 B"), "{output}");
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
fn back_key_leaves_release_selection() {
    let root = tempfile::tempdir().unwrap();
    let client = fixture(&[vec!["v2.6"]]);
    let mut prompt = Prompt::new(BackInput, Vec::new());
    assert!(matches!(
        pick_release(
            &paths(root.path()),
            &GlobalConfig::default(),
            &mut prompt,
            Provider::Dxvk,
            &client,
            false,
        )
        .unwrap(),
        Reply::Back
    ));
}

#[test]
fn more_paginates_and_can_return_to_short_list() {
    let root = tempfile::tempdir().unwrap();
    let first: Vec<String> = (1..=30).rev().map(|n| format!("v2.{n}")).collect();
    let first: Vec<&str> = first.iter().map(String::as_str).collect();
    let client = fixture(&[first, vec!["v1.9", "v1.8"]]);
    let mut prompt = Prompt::new(Script::new(&["m", "n", "p", "b", "b"]), Vec::new());
    assert!(matches!(
        pick_release(
            &paths(root.path()),
            &GlobalConfig::default(),
            &mut prompt,
            Provider::Dxvk,
            &client,
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
    pick_release(
        &paths(root.path()),
        &GlobalConfig::default(),
        &mut prompt,
        Provider::Dxvk,
        &client,
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
fn quick_selection_prints_details_once_and_completes_transfer() {
    assert_successful_flow("quick");
}

#[test]
fn picker_selection_prints_details_once_and_completes_transfer() {
    assert_successful_flow("picker");
}

#[test]
fn successful_download_child() {
    let Ok(mode) = std::env::var("WINEO_DOWNLOAD_TEST_CHILD") else {
        return;
    };
    let (name, output) = successful_flow(&mode).unwrap();
    println!("BEGIN_DOWNLOAD_OUTPUT");
    print!("{output}");
    println!("\nFINAL_NAME={name}");
}

fn assert_successful_flow(mode: &str) {
    let result = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "commands::download::tests::successful_download_child",
            "--nocapture",
        ])
        .env("WINEO_DOWNLOAD_TEST_CHILD", mode)
        .env_remove("CI")
        .env_remove("CLICOLOR")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("NO_COLOR")
        .output()
        .unwrap();
    assert!(result.status.success(), "child download failed: {result:?}");
    let stdout = String::from_utf8(result.stdout).unwrap();
    let stderr = String::from_utf8(result.stderr).unwrap();
    let transcript = stdout
        .split_once("BEGIN_DOWNLOAD_OUTPUT\n")
        .unwrap_or_else(|| panic!("missing transcript in {stdout:?}"))
        .1;
    let (transcript, final_name) = transcript
        .split_once("\nFINAL_NAME=")
        .unwrap_or_else(|| panic!("missing final name in {transcript:?}"));
    assert_eq!(final_name.lines().next(), Some("dxvk-2.6"));

    let start = transcript
        .find("v2.6\n  asset: dxvk-2.6.tar.gz (174 B)")
        .unwrap_or_else(|| panic!("missing release details in {transcript:?}"));
    let details = &transcript[start..];
    assert_eq!(details.lines().next(), Some("v2.6"));
    assert_eq!(details.matches("v2.6").count(), 1);
    assert_eq!(details.matches("asset: dxvk-2.6.tar.gz").count(), 1);
    assert_eq!(details.matches("174 B").count(), 1);
    assert_eq!(
        details
            .matches("verification: GitHub asset SHA-256")
            .count(),
        1
    );
    assert_eq!(details.matches("destination:").count(), 1);

    assert_eq!(
        stderr.matches("Downloaded 174 B / 174 B").count(),
        1,
        "{stderr}"
    );
    for progress in ["Verifying download", "Extracting build", "Installed in "] {
        assert_eq!(stderr.matches(progress).count(), 1, "{stderr}");
    }
    assert!(
        stderr
            .lines()
            .any(|line| line.starts_with("Installed in ") && line.ends_with("/dxvk-2.6")),
        "{stderr}"
    );
}

fn successful_flow(mode: &str) -> anyhow::Result<(String, String)> {
    let root = tempfile::tempdir()?;
    let paths = paths(root.path());
    let global = GlobalConfig::default();
    let client = fixture(&[vec!["v2.6"]]);
    let cancellation = Cancellation::new()?;
    let (name, output) = match mode {
        "quick" => {
            let mut output = Vec::new();
            let name = install_selected(
                &paths,
                &global,
                Selection {
                    provider: Provider::Dxvk,
                    tag: None,
                    expected_name: None,
                },
                &client,
                &cancellation,
                &mut output,
            )?;
            (name, output)
        }
        "picker" => {
            let mut prompt = Prompt::new(Script::new(&["1", ""]), Vec::new());
            let reply = pick_release(&paths, &global, &mut prompt, Provider::Dxvk, &client, false)?;
            let Reply::Value(name) = reply else {
                anyhow::bail!("fixture picker returned back");
            };
            (name, prompt.output().clone())
        }
        _ => anyhow::bail!("unknown test flow {mode}"),
    };
    anyhow::ensure!(name == "dxvk-2.6", "unexpected installed name {name}");
    let destination = paths.default_dxvk_root().join(&name);
    anyhow::ensure!(destination.join("x64/dxgi.dll").is_file());
    anyhow::ensure!(destination.join("x32/dxgi.dll").is_file());
    Ok((name, String::from_utf8(output)?))
}
