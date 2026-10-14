use std::io::Cursor;

use super::*;

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

#[test]
fn pinned_selection_uses_the_exact_release() {
    let url = "https://api.github.com/repos/doitsujin/dxvk/releases/tags/v2.6";
    let release = serde_json::json!({
        "tag_name": "v2.6",
        "assets": [{
            "name": "dxvk-2.6.tar.gz",
            "browser_download_url": "https://fixture/a",
            "size": 1,
            "digest": null
        }]
    });
    let client = Client::with_transport(Fixture(BTreeMap::from([(
        url.to_owned(),
        serde_json::to_vec(&release).unwrap(),
    )])));
    let selected = client.select_pinned(Provider::Dxvk, "dxvk-2.6").unwrap();
    assert_eq!(selected.tag, "v2.6");
    assert_eq!(selected.installed_name, "dxvk-2.6");
}

#[test]
fn canceled_download_has_a_typed_error() {
    let cancelled = Token::new();
    cancelled.cancel();
    let error = check_cancel(&cancelled).unwrap_err();
    assert!(error.is::<Error>());
    assert!(matches!(
        error.downcast_ref::<Error>(),
        Some(Error::Canceled("download"))
    ));
    assert_eq!(error.to_string(), "download canceled");
}

struct Fixture(BTreeMap<String, Vec<u8>>);
impl Transport for Fixture {
    fn get(&self, url: &str) -> Result<Box<dyn Read>> {
        Ok(Box::new(Cursor::new(
            self.0
                .get(url)
                .with_context(|| format!("unexpected fixture URL {url}"))?
                .clone(),
        )))
    }
}
fn upstream(tag: &str, names: &[&str]) -> UpstreamRelease {
    UpstreamRelease {
        tag_name: tag.into(),
        draft: false,
        prerelease: false,
        assets: names
            .iter()
            .map(|name| Asset {
                name: (*name).into(),
                size: 1,
                digest: None,
                browser_download_url: format!("https://fixtures/{name}"),
            })
            .collect(),
    }
}
fn tar_bytes(entries: &[(&str, &[u8], u32)], links: &[(&str, &str, bool)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, bytes, mode) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(*mode);
        header.set_cksum();
        builder.append_data(&mut header, path, *bytes).unwrap();
    }
    for (path, target, hard) in links {
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o777);
        header.set_entry_type(if *hard {
            tar::EntryType::Link
        } else {
            tar::EntryType::Symlink
        });
        header.set_link_name(target).unwrap();
        header.set_cksum();
        builder.append_data(&mut header, path, &[][..]).unwrap();
    }
    builder.into_inner().unwrap()
}
fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gzip.write_all(bytes).unwrap();
    gzip.finish().unwrap()
}
fn extract_bytes(bytes: &[u8], name: &str, root: &Path) -> Result<()> {
    let archive = root.parent().unwrap().join("archive");
    fs::write(&archive, bytes)?;
    extract(&archive, name, root, &Token::new())
}
fn fixture_paths(root: &Path) -> Paths {
    Paths::from_lookup(|key| match key {
        "HOME" => Some(root.as_os_str().to_owned()),
        _ => None,
    })
    .unwrap()
}
#[test]
fn provider_assets_cover_historical_and_architecture_suffixes() {
    for (provider, tag, filename) in [
        (Provider::GeProton, "GE-Proton9-19", "GE-Proton9-19.tar.gz"),
        (
            Provider::GeProton,
            "GE-Proton11-5",
            "GE-Proton11-5-x86_64.tar.gz",
        ),
        (
            Provider::ProtonCachyos,
            "cachyos-10.0-20260425-slr",
            "proton-cachyos-10.0-20260425-slr-x86_64.tar.xz",
        ),
        (
            Provider::ProtonCachyos,
            "cachyos-10.0-20260425-slr",
            "proton-cachyos-10.0-20260425-slr-x86-64.tar.xz",
        ),
        (Provider::Dxvk, "v2.6", "dxvk-2.6.tar.gz"),
        (Provider::Vkd3d, "v2.14", "vkd3d-proton-2.14.tar.zst"),
        (Provider::Vkd3d, "v2.10", "vkd3d-proton-2.10.tar.gz"),
    ] {
        assert_eq!(
            select_asset(provider, upstream(tag, &[filename]))
                .unwrap()
                .unwrap()
                .asset_name,
            filename
        );
    }
    assert!(
        select_asset(
            Provider::Dxvk,
            upstream(
                "v2.6",
                &["dxvk-native-2.6.tar.gz", "dxvk-2.6-aarch64.tar.gz"]
            )
        )
        .unwrap()
        .is_none()
    );
    assert!(
        select_asset(
            Provider::ProtonCachyos,
            upstream(
                "x",
                &[
                    "proton-cachyos-10-slr-x86_64-v3.tar.xz",
                    "proton-cachyos-10-slr-aarch64.tar.xz"
                ]
            )
        )
        .unwrap()
        .is_none()
    );
    assert!(
        select_asset(
            Provider::GeProton,
            upstream(
                "GE-Proton11-5",
                &["GE-Proton11-5.tar.gz", "GE-Proton11-5-x86_64.tar.gz"]
            )
        )
        .is_err()
    );
}
#[test]
fn checksums_match_the_exact_asset_and_fail_closed() {
    let sha = "a".repeat(64);
    assert_eq!(
        parse_checksum(
            &format!("{sha} *build.tar.gz\n"),
            "build.tar.gz",
            "sha256sums"
        )
        .unwrap(),
        format!("sha256:{sha}")
    );
    assert!(parse_checksum(&format!("{sha} other.tar.gz"), "build.tar.gz", "checksums").is_err());
    assert!(
        parse_checksum(
            &format!("{sha} build.tar.gz\n{sha} build.tar.gz"),
            "build.tar.gz",
            "checksums"
        )
        .is_err()
    );
    assert!(parse_checksum("invalid build.tar.gz", "build.tar.gz", "checksums").is_err());
    assert!(validate_digest("sha1:abc", "sha256").is_err());
}
#[test]
fn extraction_preserves_modes_internal_symlinks_and_hardlinks() {
    use std::os::unix::fs::MetadataExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("out");
    fs::create_dir(&root).unwrap();
    let bytes = gzip(&tar_bytes(
        &[("build/bin/program", b"abc", 0o755)],
        &[
            ("build/run", "bin/program", false),
            ("build/hard", "build/bin/program", true),
        ],
    ));
    extract_bytes(&bytes, "b.tar.gz", &root).unwrap();
    assert_eq!(fs::read(root.join("build/run")).unwrap(), b"abc");
    assert_eq!(
        fs::metadata(root.join("build/run"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(
        fs::metadata(root.join("build/hard")).unwrap().ino(),
        fs::metadata(root.join("build/bin/program")).unwrap().ino()
    );
}
#[test]
fn extraction_rejects_escaping_links_duplicates_and_special_files() {
    for links in [
        vec![("build/link", "../../escape", false)],
        vec![("build/link", "/tmp/escape", false)],
        vec![("build/a", "b", false), ("build/b", "a", false)],
        vec![
            ("build/link", "../dir", false),
            ("build/other", "link/../../escape", false),
        ],
        vec![("build/hard", "../escape", true)],
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("out");
        fs::create_dir(&root).unwrap();
        assert!(extract_bytes(&gzip(&tar_bytes(&[], &links)), "b.tar.gz", &root).is_err());
    }
    assert!(safe_path(Path::new("../escape")).is_err());
    assert!(safe_path(Path::new("/escape")).is_err());
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("out");
    fs::create_dir(&root).unwrap();
    assert!(
        extract_bytes(
            &gzip(&tar_bytes(
                &[("file", b"a", 0o644), ("file", b"b", 0o644)],
                &[]
            )),
            "b.tar.gz",
            &root
        )
        .is_err()
    );
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("out");
    fs::create_dir(&root).unwrap();
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Fifo);
    header.set_size(0);
    header.set_mode(0o600);
    header.set_cksum();
    builder.append_data(&mut header, "fifo", &[][..]).unwrap();
    assert!(extract_bytes(&gzip(&builder.into_inner().unwrap()), "b.tar.gz", &root).is_err());
}
#[test]
fn gzip_xz_and_zstd_are_supported() {
    let bytes = tar_bytes(&[("file", b"contents", 0o644)], &[]);
    let mut xz = xz2::write::XzEncoder::new(Vec::new(), 1);
    xz.write_all(&bytes).unwrap();
    for (name, compressed) in [
        ("b.tar.gz", gzip(&bytes)),
        ("b.tar.xz", xz.finish().unwrap()),
        (
            "b.tar.zst",
            zstd::stream::encode_all(bytes.as_slice(), 1).unwrap(),
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("out");
        fs::create_dir(&root).unwrap();
        extract_bytes(&compressed, name, &root).unwrap();
        assert_eq!(fs::read(root.join("file")).unwrap(), b"contents");
    }
}
fn install_fixture(checksum: bool) -> (Client<Fixture>, Release) {
    let bytes = gzip(&tar_bytes(
        &[
            ("dxvk-2.6/x64/dxgi.dll", b"wide", 0o644),
            ("dxvk-2.6/x32/dxgi.dll", b"narrow", 0o644),
        ],
        &[],
    ));
    let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
    let mut release = upstream("v2.6", &["dxvk-2.6.tar.gz"]);
    release.assets[0].size = bytes.len() as u64;
    let mut fixture = BTreeMap::from([("https://fixtures/dxvk-2.6.tar.gz".into(), bytes.clone())]);
    if checksum {
        release.assets.push(Asset {
            name: "dxvk-2.6.sha512sum".into(),
            browser_download_url: "https://fixtures/sum".into(),
            size: 1,
            digest: None,
        });
        fixture.insert(
            "https://fixtures/sum".into(),
            format!("{:x}  dxvk-2.6.tar.gz\n", Sha512::digest(&bytes)).into_bytes(),
        );
    } else {
        release.assets[0].digest = Some(digest);
    }
    (
        Client::with_transport(Fixture(fixture)),
        select_asset(Provider::Dxvk, release).unwrap().unwrap(),
    )
}
#[test]
fn installation_is_verified_atomic_and_idempotent() {
    for checksum in [false, true] {
        let (client, release) = install_fixture(checksum);
        let temp = tempfile::tempdir().unwrap();
        let paths = fixture_paths(temp.path());
        let destination = client
            .install(
                &paths,
                &GlobalConfig::default(),
                &release,
                &Token::new(),
                |_| {},
            )
            .unwrap();
        assert!(destination.join("x64/dxgi.dll").is_file());
        let receipt: Receipt =
            serde_json::from_slice(&fs::read(destination.join(RECEIPT)).unwrap()).unwrap();
        assert_eq!(receipt.version, "v2.6");
        let mut already = false;
        client
            .install(
                &paths,
                &GlobalConfig::default(),
                &release,
                &Token::new(),
                |p| {
                    already |= matches!(p, Progress::AlreadyInstalled(_));
                },
            )
            .unwrap();
        assert!(already);
    }
}
#[test]
fn cancellation_on_either_side_of_publication_matches_installed_state() {
    let (client, release) = install_fixture(false);
    let temp = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temp.path());
    let global = GlobalConfig::default();
    let destination = release.destination(&paths, &global).unwrap();
    let cancelled = Token::new();
    let result = client.install_with_phase(
        &paths,
        &global,
        &release,
        &cancelled,
        |_| {},
        |phase| {
            if matches!(phase, InstallPhase::BeforePublication) {
                cancelled.cancel();
            }
        },
    );
    assert!(result.unwrap_err().to_string().contains("canceled"));
    assert!(!destination.exists());
    let parent = destination.parent().unwrap();
    assert!(fs::read_dir(parent).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(&stage_prefix(&destination))
    }));

    let after_cancelled = Token::new();
    let result = client.install_with_phase(
        &paths,
        &global,
        &release,
        &after_cancelled,
        |_| {},
        |phase| {
            if matches!(phase, InstallPhase::AfterPublication) {
                after_cancelled.cancel();
            }
        },
    );
    assert_eq!(result.unwrap(), destination);
    assert!(after_cancelled.is_canceled());
    assert_eq!(fs::read(destination.join("x64/dxgi.dll")).unwrap(), b"wide");
    assert!(destination.join(RECEIPT).is_file());
}
#[test]
fn abandoned_stage_cleanup_is_locked_and_scoped_to_destination() {
    let (client, release) = install_fixture(false);
    let (racing_client, _) = install_fixture(false);
    let temp = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temp.path());
    let global = GlobalConfig::default();
    let destination = destination_identity(&release.destination(&paths, &global).unwrap()).unwrap();
    let parent = destination.parent().unwrap();
    fs::create_dir_all(parent).unwrap();
    let abandoned = parent.join(format!("{}abandoned", stage_prefix(&destination)));
    let other_destination = parent.join("other-build");
    let unrelated = parent.join(format!("{}active", stage_prefix(&other_destination)));
    let completed = parent.join("completed-build");
    for dir in [&abandoned, &unrelated, &completed] {
        fs::create_dir(dir).unwrap();
        fs::write(dir.join("marker"), dir.as_os_str().as_encoded_bytes()).unwrap();
    }
    let mut checked_active_stage = false;
    client
        .install(&paths, &global, &release, &Token::new(), |phase| {
            if matches!(phase, Progress::Downloading { .. }) && !checked_active_stage {
                checked_active_stage = true;
                let active = fs::read_dir(parent)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .find(|path| {
                        path.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with(&stage_prefix(&destination))
                    })
                    .unwrap();
                assert!(
                    racing_client
                        .install(&paths, &global, &release, &Token::new(), |_| {})
                        .unwrap_err()
                        .to_string()
                        .contains("another install")
                );
                assert!(active.exists());
            }
        })
        .unwrap();
    assert!(checked_active_stage);
    assert!(!abandoned.exists());
    assert!(unrelated.join("marker").is_file());
    assert!(completed.join("marker").is_file());
    assert!(destination.join(RECEIPT).is_file());

    fs::create_dir(&abandoned).unwrap();
    fs::write(abandoned.join("marker"), b"orphan").unwrap();
    client
        .install(&paths, &global, &release, &Token::new(), |phase| {
            assert!(matches!(phase, Progress::AlreadyInstalled(_)));
        })
        .unwrap();
    assert!(!abandoned.exists());
    assert!(unrelated.join("marker").is_file());
    assert!(completed.join("marker").is_file());
    assert!(destination.join(RECEIPT).is_file());
}
#[test]
fn corruption_cancellation_and_collision_leave_no_partial_install() {
    let (client, mut release) = install_fixture(false);
    let temp = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temp.path());
    let global = GlobalConfig::default();
    let destination = release.destination(&paths, &global).unwrap();
    let cancelled = Token::new();
    cancelled.cancel();
    assert!(
        client
            .install(&paths, &global, &release, &cancelled, |_| {})
            .is_err()
    );
    assert!(!destination.exists());
    release.asset.digest = Some(format!("sha256:{}", "0".repeat(64)));
    assert!(
        client
            .install(&paths, &global, &release, &Token::new(), |_| {})
            .is_err()
    );
    assert!(!destination.exists());
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("mine"), "preserved").unwrap();
    assert!(
        client
            .install(&paths, &global, &release, &Token::new(), |_| {})
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(destination.join("mine")).unwrap(),
        "preserved"
    );
}
#[test]
fn destination_lock_and_publication_refuse_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let dest = temp.path().join("build");
    let guard = InstallLock::acquire(&dest).unwrap();
    assert!(InstallLock::acquire(&dest).is_err());
    drop(guard);
    let _guard = InstallLock::acquire(&dest).unwrap();
    fs::create_dir(&dest).unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    assert!(publish(&source, &dest).is_err());
    assert!(source.is_dir());
}
#[test]
fn install_lock_releases_even_if_a_forked_child_still_holds_the_fd() {
    let temp = tempfile::tempdir().unwrap();
    let dest = temp.path().join("build");
    let guard = InstallLock::acquire(&dest).unwrap();
    // Stands in for a forked child's copy of the fd, kept alive past the drop.
    let clone = guard.file.try_clone().unwrap();
    drop(guard);
    assert!(InstallLock::acquire(&dest).is_ok());
    drop(clone);
}
#[test]
fn metadata_selection_obeys_stability_pagination_and_exact_tags() {
    let mut releases = Vec::new();
    for index in 0..30 {
        let mut release = upstream(&format!("v3.{index}"), &["unsupported.zip"]);
        release.prerelease = true;
        releases
            .push(serde_json::json!({"tag_name":release.tag_name,"prerelease":true,"assets":[]}));
    }
    let page2 = serde_json::json!([{"tag_name":"v2.6","assets":[{"name":"dxvk-2.6.tar.gz","browser_download_url":"https://fixture/a","size":1,"digest":null}]}]);
    let fixture = Fixture(BTreeMap::from([
        (
            "https://api.github.com/repos/doitsujin/dxvk/releases?per_page=30&page=1".into(),
            serde_json::to_vec(&releases).unwrap(),
        ),
        (
            "https://api.github.com/repos/doitsujin/dxvk/releases?per_page=30&page=2".into(),
            serde_json::to_vec(&page2).unwrap(),
        ),
    ]));
    let client = Client::with_transport(fixture);
    assert!(client.list(Provider::Dxvk, 1).unwrap().has_more);
    assert_eq!(client.select(Provider::Dxvk, None).unwrap().tag, "v2.6");
    assert!(client.select(Provider::Dxvk, Some("v0.0")).is_err());
}
#[test]
fn existing_destination_symlink_is_not_followed() {
    let (client, release) = install_fixture(false);
    let temp = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temp.path());
    let global = GlobalConfig::default();
    let destination = client
        .install(&paths, &global, &release, &Token::new(), |_| {})
        .unwrap();
    let elsewhere = temp.path().join("elsewhere");
    fs::rename(&destination, &elsewhere).unwrap();
    symlink(&elsewhere, &destination).unwrap();
    let error = client
        .install(&paths, &global, &release, &Token::new(), |_| {})
        .unwrap_err();
    assert!(error.to_string().contains("symlink"));
    assert!(elsewhere.join(RECEIPT).is_file());
}

#[test]
fn missing_malformed_and_disagreeing_verification_is_rejected() {
    let (client, mut release) = install_fixture(true);
    release.asset.digest = Some(format!("sha256:{}", "0".repeat(64)));
    let temp = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temp.path());
    let global = GlobalConfig::default();
    assert!(
        client
            .install(&paths, &global, &release, &Token::new(), |_| {})
            .is_err()
    );
    assert!(!release.destination(&paths, &global).unwrap().exists());
    release.asset.digest = Some("sha1:invalid".into());
    assert!(client.expected_digests(&release).is_err());
    release.asset.digest = None;
    release.checksums.clear();
    assert!(client.expected_digests(&release).is_err());
}

#[test]
fn truncated_download_and_midstream_cancel_remove_staging() {
    let (client, mut release) = install_fixture(false);
    let temp = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temp.path());
    let global = GlobalConfig::default();
    let destination = release.destination(&paths, &global).unwrap();
    release.size += 1;
    assert!(
        client
            .install(&paths, &global, &release, &Token::new(), |_| {})
            .is_err()
    );
    release.size -= 1;
    let cancelled = Token::new();
    assert!(
        client
            .install(&paths, &global, &release, &cancelled, |p| {
                if matches!(p, Progress::Downloading { .. }) {
                    cancelled.cancel();
                }
            })
            .is_err()
    );
    assert!(!destination.exists());
    assert!(
        fs::read_dir(destination.parent().unwrap())
            .unwrap()
            .all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".wineo-stage-"))
    );
}
#[test]
fn blocking_http_body_reads_without_an_async_runtime() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        let _ = stream.read(&mut request).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nfixture")
            .unwrap();
    });
    let client = http_client(false, Duration::from_secs(2)).unwrap();
    let mut response = client
        .get(format!("http://{address}/fixture"))
        .send()
        .unwrap();
    let mut body = String::new();
    response.read_to_string(&mut body).unwrap();
    assert_eq!(body, "fixture");
    server.join().unwrap();
    assert!(Https(client).get("http://example.invalid/fixture").is_err());
}

#[test]
fn blocking_http_stalled_body_has_a_bounded_read_timeout() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        let _ = stream.read(&mut request).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nabc")
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
    });
    let client = http_client(false, Duration::from_millis(100)).unwrap();
    let mut response = client
        .get(format!("http://{address}/fixture"))
        .send()
        .unwrap();
    let started = std::time::Instant::now();
    assert!(response.read_exact(&mut [0; 8]).is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
    server.join().unwrap();
}
#[test]
fn streaming_can_outlast_the_per_read_timeout() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        let _ = stream.read(&mut request).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\n")
            .unwrap();
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(50));
            stream.write_all(b"x").unwrap();
            stream.flush().unwrap();
        }
    });
    let client = http_client(false, Duration::from_millis(200)).unwrap();
    let started = std::time::Instant::now();
    let mut response = client
        .get(format!("http://{address}/fixture"))
        .send()
        .unwrap();
    let mut body = [0; 8];
    response.read_exact(&mut body).unwrap();
    assert_eq!(&body, b"xxxxxxxx");
    assert!(started.elapsed() > Duration::from_millis(200));
    server.join().unwrap();
}
#[test]
fn ge_transition_releases_prefer_the_checksum_backed_gzip_package() {
    // Official GE-Proton10-28 publishes both formats; its .sha512sum names gzip.
    for names in [
        ["GE-Proton10-28.tar.zst", "GE-Proton10-28.tar.gz"],
        ["GE-Proton10-28.tar.gz", "GE-Proton10-28.tar.zst"],
    ] {
        let release = select_asset(Provider::GeProton, upstream("GE-Proton10-28", &names))
            .unwrap()
            .unwrap();
        assert_eq!(release.asset_name, "GE-Proton10-28.tar.gz");
    }
    assert!(
        select_asset(
            Provider::GeProton,
            upstream(
                "GE-Proton10-28",
                &["GE-Proton10-28.tar.gz", "GE-Proton10-28-x86_64.tar.zst"]
            )
        )
        .is_err()
    );
    assert!(
        select_asset(
            Provider::GeProton,
            upstream(
                "GE-Proton10-28",
                &["GE-Proton10-28.tar.gz", "GE-Proton10-28.tar.gz"]
            )
        )
        .is_err()
    );
}

#[test]
fn a_lazy_value_is_built_once_and_a_failure_is_retried() {
    let builds = std::cell::Cell::new(0);
    let lazy = Lazy::<u32>::new();
    let failed = lazy.get_or_try_init(|| {
        builds.set(builds.get() + 1);
        Err::<u32, _>("no")
    });
    assert_eq!(failed, Err("no"));
    for _ in 0..5 {
        let value = lazy.get_or_try_init(|| {
            builds.set(builds.get() + 1);
            Ok::<_, &str>(7)
        });
        assert_eq!(value, Ok(&7));
    }
    // One failed build, then one that stuck for every later item.
    assert_eq!(builds.get(), 2);
}

#[test]
fn a_lazy_https_hands_every_item_the_same_client() {
    let lazy = Lazy::<Https>::new();
    let first: *const Https = lazy.https().unwrap();
    let second: *const Https = lazy.https().unwrap();
    assert_eq!(first, second);
}
