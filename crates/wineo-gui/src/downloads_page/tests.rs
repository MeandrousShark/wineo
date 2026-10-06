use super::*;
use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::os::unix::fs::PermissionsExt;

use iced::event::Status;
use sha2::{Digest, Sha256};
use wineo_core::build_lock::Lock;
use wineo_core::components::Component;
use wineo_core::download::{Asset, Client, Transport, UpstreamRelease, select_asset};
use wineo_core::paths::Paths;

fn fixture() -> (tempfile::TempDir, App) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.as_os_str().to_owned()))
        .unwrap();
    (root, App::boot(paths).0)
}

pub(crate) fn generation(app: &App) -> u64 {
    app.downloads_page.generation
}

fn send(event: Event, app: &mut App) {
    let _ = app.update(message(event));
}

fn load(app: &mut App) {
    let snapshot = downloads_work::read(&app.paths);
    send(Event::Loaded(app.downloads_page.generation, snapshot), app);
}

fn open(app: &mut App) {
    let _ = app.update(Message::Section(sidebar::Section::Downloads));
    load(app);
}

fn runner(paths: &Paths, name: &str, proton: bool) {
    let dir = paths.default_runner_root().join(name);
    let programs = if proton {
        vec!["proton", "files/bin/wine", "files/bin/wineserver"]
    } else {
        vec!["bin/wine", "bin/wineserver"]
    };
    for program in programs {
        let file = dir.join(program);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn component(paths: &Paths, kind: Component, name: &str) {
    let root = match kind {
        Component::Dxvk => paths.default_dxvk_root(),
        Component::Vkd3d => paths.default_vkd3d_root(),
    };
    let (wide, narrow) = kind.arch_dirs();
    let marker = if kind == Component::Dxvk {
        "dxgi.dll"
    } else {
        "d3d12.dll"
    };
    for arch in [wide, narrow] {
        let dir = root.join(name).join(arch);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(marker), "fake dll").unwrap();
    }
}

fn config(paths: &Paths, name: &str, settings: &str) {
    let file = paths.environment_config(name);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(
        file,
        format!(
            "prefix = '{}'\narch = 'win64'\n{settings}",
            paths.default_prefix_root().join(name).display()
        ),
    )
    .unwrap();
}

fn build(app: &App, kind: Kind, name: &str) -> Build {
    app.downloads_page
        .snapshot()
        .unwrap()
        .builds
        .iter()
        .find(|build| build.kind == kind && build.name == name)
        .unwrap()
        .clone()
}

fn review(app: &mut App, build: &Build) {
    send(Event::Review(build.id()), app);
    assert!(app.downloads_confirm_open());
    assert_eq!(app.downloads_confirm_controls(), vec![CANCEL_ID]);
    send(
        Event::Measured(
            app.downloads_page.review_generation,
            downloads_work::size(&app.paths, build),
        ),
        app,
    );
    assert_eq!(
        app.downloads_confirm_controls(),
        vec![CANCEL_ID, CONFIRM_ID]
    );
}

#[test]
fn installed_builds_show_uses_aliases_unused_and_core_skip_reasons() {
    let (_root, mut app) = fixture();
    runner(&app.paths, "GE-Proton9-20", true);
    runner(&app.paths, "proton-cachyos", true);
    runner(&app.paths, "wine", false);
    runner(&app.paths, "wine-tkg", false);
    component(&app.paths, Component::Dxvk, "2.5.3");
    component(&app.paths, Component::Vkd3d, "2.14");
    component(&app.paths, Component::Vkd3d, "2.13");
    fs::create_dir_all(app.paths.default_runner_root().join("broken")).unwrap();
    fs::create_dir_all(app.paths.default_dxvk_root().join("half")).unwrap();
    fs::create_dir_all(app.paths.default_vkd3d_root().join("half")).unwrap();
    config(
        &app.paths,
        "hades",
        "runner = 'GE-Proton-Latest'\ndxvk = 'latest'\n",
    );
    config(&app.paths, "celeste", "runner = 'GE-Proton9-20'\n");
    open(&mut app);
    let ge = build(&app, Kind::Runner, "GE-Proton9-20");
    assert_eq!(ge.users, ["celeste", "hades (GE-Proton-Latest)"]);
    assert!(!ge.unused());
    assert_eq!(
        build(&app, Kind::Dxvk, "2.5.3").uses_label(),
        "used by hades (latest), template gaming (latest)"
    );
    assert_eq!(
        build(&app, Kind::Runner, "proton-cachyos").users,
        ["template gaming (proton)"]
    );
    let unused = build(&app, Kind::Runner, "wine-tkg");
    assert!(unused.unused());
    assert_eq!(unused.uses_label(), "unused");
    assert!(!build(&app, Kind::Vkd3d, "2.14").unused());
    assert!(build(&app, Kind::Vkd3d, "2.13").unused());
    let snapshot = app.downloads_page.snapshot().unwrap();
    assert!(
        snapshot
            .skipped
            .iter()
            .any(|reason| reason == "Runners / broken: no bin/wine and no proton script")
    );
    assert!(
        snapshot
            .skipped
            .iter()
            .any(|reason| reason == "DXVK / half: no x64/dxgi.dll")
    );
    assert!(
        snapshot
            .skipped
            .iter()
            .any(|reason| reason == "vkd3d-proton / half: no x64/d3d12.dll")
    );
    assert!(!snapshot.builds.iter().any(|build| build.name == "wine"));
    assert!(!Build::new(Kind::Runner, "wine").unused());
    send(Event::Review(ge.id()), &mut app);
    assert!(!app.downloads_confirm_open());
    let _ = app.downloads_view();
}

#[test]
fn global_default_is_a_use_and_failed_uses_disable_delete() {
    let (_root, mut app) = fixture();
    runner(&app.paths, "custom", false);
    let global = app.paths.global_config();
    fs::create_dir_all(global.parent().unwrap()).unwrap();
    fs::write(global, "[defaults]\nrunner = 'custom'\n").unwrap();
    open(&mut app);
    assert_eq!(
        build(&app, Kind::Runner, "custom").users,
        ["global default runner", "template gaming (proton)"]
    );
    let mut unknown = Build::new(Kind::Runner, "unknown");
    unknown.error = Some("failed to read configs".into());
    assert!(!unknown.unused());
}

#[test]
fn delete_confirms_size_then_refreshes_and_preserves_skipped_file_warnings() {
    let (_root, mut app) = fixture();
    component(&app.paths, Component::Dxvk, "2.5.3");
    component(&app.paths, Component::Dxvk, "2.6");
    let broken = app.paths.environment_config("broken");
    fs::create_dir_all(broken.parent().unwrap()).unwrap();
    fs::write(&broken, "invalid config").unwrap();
    open(&mut app);
    let unused = build(&app, Kind::Dxvk, "2.5.3");
    assert!(
        app.downloads_page
            .snapshot()
            .unwrap()
            .warnings
            .iter()
            .any(|warning| warning.contains("broken.toml"))
    );
    send(Event::Review(unused.id()), &mut app);
    send(Event::Confirm, &mut app);
    assert!(app.change.is_none());
    send(
        Event::Measured(
            app.downloads_page.review_generation,
            downloads_work::size(&app.paths, &unused),
        ),
        &mut app,
    );
    assert_eq!(app.downloads_page.review.as_ref().unwrap().1, Some(16));
    let _ = app.downloads_confirm_view();
    send(Event::Confirm, &mut app);
    assert!(matches!(
        app.change,
        Some(removal::Change::Downloads(Change::Delete(_)))
    ));
    let generation = app.change_generation;
    let removed = downloads_work::remove(&app.paths, &unused).unwrap();
    let warnings = downloads_work::warnings(&app.paths, &removed.errors);
    assert_eq!(removed.bytes, 16);
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("broken.toml"))
    );
    let page_generation = app.downloads_page.generation;
    send(
        Event::Deleted(generation, Ok((removed.bytes, warnings))),
        &mut app,
    );
    assert_eq!(
        app.downloads_page.generation,
        page_generation.wrapping_add(1)
    );
    assert!(app.change.is_none());
    assert!(app.downloads_page.loading);
    load(&mut app);
    assert!(
        !app.downloads_page
            .snapshot()
            .unwrap()
            .builds
            .iter()
            .any(|build| build.name == "2.5.3")
    );
    assert_eq!(app.downloads_page.snapshot().unwrap().builds.len(), 1);
    assert!(
        app.downloads_page
            .notice
            .as_ref()
            .unwrap()
            .contains("16 B freed")
    );
    assert!(
        app.downloads_page
            .warnings
            .iter()
            .any(|warning| warning.contains("broken.toml"))
    );
    assert_eq!(crate::size::format_bytes(1_288_490_189), "1.2 GiB");
}

#[test]
fn delete_error_after_move_stays_visible_after_refresh_removes_the_build() {
    let (_root, mut app) = fixture();
    runner(&app.paths, "unused", false);
    open(&mut app);
    let unused = build(&app, Kind::Runner, "unused");
    send(Event::Review(unused.id()), &mut app);
    send(
        Event::Measured(app.downloads_page.review_generation, Ok(1)),
        &mut app,
    );
    send(Event::Confirm, &mut app);
    let generation = app.change_generation;

    let build_dir = app.paths.default_runner_root().join(&unused.name);
    let staged = app
        .paths
        .default_runner_root()
        .join(".wineo-remove-unused-Ab12Cd");
    fs::create_dir_all(&staged).unwrap();
    fs::rename(&build_dir, staged.join("build")).unwrap();
    let error = format!("failed to delete {}", staged.display());
    send(Event::Deleted(generation, Err(error.clone())), &mut app);
    load(&mut app);

    assert!(
        app.downloads_page
            .snapshot()
            .unwrap()
            .builds
            .iter()
            .all(|build| build.id() != unused.id())
    );
    assert_eq!(
        app.downloads_page.feedback(),
        vec![(format!("{}: {error}", unused.id()), true)]
    );
    let _ = app.downloads_view();
}

#[test]
fn references_added_after_review_and_held_build_locks_refuse_on_the_row() {
    for held in [false, true] {
        let (_root, mut app) = fixture();
        runner(&app.paths, "custom", false);
        open(&mut app);
        let unused = build(&app, Kind::Runner, "custom");
        review(&mut app, &unused);
        let lock = Lock::new(&app.paths, Kind::Runner, "custom");
        let _guard = if held {
            Some(lock.shared().unwrap())
        } else {
            config(&app.paths, "hades", "runner = 'custom'\n");
            None
        };
        send(Event::Confirm, &mut app);
        let error = downloads_work::remove(&app.paths, &unused).unwrap_err();
        assert!(error.contains("custom is used by"), "{error}");
        if !held {
            assert!(error.contains("hades"));
        }
        send(
            Event::Deleted(app.change_generation, Err(error.clone())),
            &mut app,
        );
        load(&mut app);
        assert_eq!(
            app.downloads_page.row_errors.get(&unused.id()),
            Some(&error)
        );
        assert!(app.paths.default_runner_root().join("custom").is_dir());
        let _ = app.downloads_view();
    }
}

#[test]
fn open_sweeps_each_root_best_effort_and_refresh_ignores_stale_reads() {
    let (_root, mut app) = fixture();
    let roots = [
        app.paths.default_runner_root(),
        app.paths.default_dxvk_root(),
        app.paths.default_vkd3d_root(),
    ];
    for root in &roots {
        let staged = root.join(".wineo-remove-abandoned-Ab12Cd");
        fs::create_dir_all(&staged).unwrap();
        fs::write(staged.join("partial"), "leftover").unwrap();
    }
    let held = Lock::new(&app.paths, Kind::Dxvk, "abandoned")
        .shared()
        .unwrap();
    open(&mut app);
    assert!(!roots[0].join(".wineo-remove-abandoned-Ab12Cd").exists());
    assert!(roots[1].join(".wineo-remove-abandoned-Ab12Cd").exists());
    assert!(!roots[2].join(".wineo-remove-abandoned-Ab12Cd").exists());
    drop(held);
    let old = app.downloads_page.generation;
    send(Event::Refresh, &mut app);
    send(Event::Loaded(old, Err("stale".into())), &mut app);
    assert!(app.downloads_page.loading);
    load(&mut app);
    assert!(!roots[1].join(".wineo-remove-abandoned-Ab12Cd").exists());
    assert!(!app.downloads_page.loading);
    let _ = app.update(Message::Section(sidebar::Section::Library));
    runner(&app.paths, "new", false);
    open(&mut app);
    assert_eq!(build(&app, Kind::Runner, "new").name, "new");
}

#[test]
fn generic_change_reload_skips_downloads_until_the_page_opens() {
    let (_root, mut app) = fixture();
    open(&mut app);
    let generation = app.downloads_page.generation;

    let _ = app.reload_after_change();
    assert_eq!(app.downloads_page.generation, generation);

    let _ = app.update(Message::Section(sidebar::Section::Library));
    let _ = app.update(Message::Section(sidebar::Section::Downloads));
    assert_eq!(app.downloads_page.generation, generation.wrapping_add(1));
}

#[test]
fn confirm_keys_isolate_navigation_and_late_measurement_is_ignored() {
    let (_root, mut app) = fixture();
    runner(&app.paths, "unused", false);
    open(&mut app);
    let unused = build(&app, Kind::Runner, "unused");
    review(&mut app, &unused);
    let generation = app.downloads_page.review_generation;
    crate::shortcuts::tests::press(&mut app, "1", Modifiers::CTRL);
    assert_eq!(app.section, sidebar::Section::Downloads);
    let _ = app.update(Message::Key(
        Key::Named(Named::Tab),
        Modifiers::empty(),
        Status::Ignored,
    ));
    assert_eq!(focus_button::take_cycles().len(), 1);
    let _ = app.update(Message::Key(
        Key::Named(Named::Escape),
        Modifiers::empty(),
        Status::Ignored,
    ));
    assert!(!app.downloads_confirm_open());
    send(Event::Measured(generation, Ok(1)), &mut app);
    assert!(app.downloads_page.review.is_none());
    send(Event::Review(unused.id()), &mut app);
    assert!(app.downloads_page.review.as_ref().unwrap().1.is_none());
    send(Event::Measured(generation, Ok(1)), &mut app);
    assert!(app.downloads_page.review.as_ref().unwrap().1.is_none());
}

#[test]
fn downloads_shortcut_shares_overlay_gating_and_settings_keeps_comma() {
    let (_root, mut app) = fixture();
    let (messages, status) =
        crate::shortcuts::tests::input_messages("search", "4", Modifiers::CTRL);
    assert_eq!(status, Status::Captured);
    assert!(matches!(
        messages.as_slice(),
        [Message::Section(sidebar::Section::Downloads)]
    ));
    app.search = "search".into();
    crate::shortcuts::tests::press(&mut app, "4", Modifiers::CTRL);
    assert_eq!(app.section, sidebar::Section::Downloads);
    assert_eq!(app.search, "search");
    let _ = app.sidebar();
    let _ = app.update(Message::Section(sidebar::Section::Library));
    app.flow = Some(crate::flow::Page::choose(app.paths.clone()));
    crate::shortcuts::tests::press(&mut app, "4", Modifiers::CTRL);
    assert_eq!(app.section, sidebar::Section::Library);
    app.flow = None;
    app.close_prompt = true;
    crate::shortcuts::tests::press(&mut app, "4", Modifiers::CTRL);
    assert_eq!(app.section, sidebar::Section::Library);
    app.close_prompt = false;
    crate::shortcuts::tests::press(&mut app, ",", Modifiers::CTRL);
    assert!(app.global_settings_open());
    for modifiers in [
        Modifiers::empty(),
        Modifiers::CTRL | Modifiers::SHIFT,
        Modifiers::CTRL | Modifiers::ALT,
    ] {
        assert_eq!(
            sidebar::shortcut(&Key::Character("4".into()), modifiers),
            None
        );
    }
}

struct Fake(BTreeMap<String, Vec<u8>>);
impl Transport for Fake {
    fn get(&self, url: &str) -> anyhow::Result<Box<dyn Read>> {
        Ok(Box::new(Cursor::new(
            self.0
                .get(url)
                .ok_or_else(|| anyhow::anyhow!("unexpected fake URL {url}"))?
                .clone(),
        )))
    }
}

fn upstream(provider: Provider, tag: &str) -> UpstreamRelease {
    let name = match provider {
        Provider::GeProton => format!("{tag}.tar.gz"),
        Provider::ProtonCachyos => format!("proton-cachyos-{tag}-slr-x86_64.tar.xz"),
        Provider::Dxvk => format!("dxvk-{}.tar.gz", tag.trim_start_matches('v')),
        Provider::Vkd3d => format!("vkd3d-proton-{}.tar.zst", tag.trim_start_matches('v')),
    };
    UpstreamRelease {
        tag_name: tag.into(),
        draft: false,
        prerelease: false,
        assets: vec![Asset {
            browser_download_url: format!("https://fake/{name}"),
            name,
            size: 1,
            digest: None,
        }],
    }
}

fn catalog_url(provider: Provider, page: u32) -> String {
    let repo = match provider {
        Provider::GeProton => "GloriousEggroll/proton-ge-custom",
        Provider::ProtonCachyos => "CachyOS/proton-cachyos",
        Provider::Dxvk => "doitsujin/dxvk",
        Provider::Vkd3d => "HansKristian-Work/vkd3d-proton",
    };
    format!("https://api.github.com/repos/{repo}/releases?per_page=30&page={page}")
}

fn page_bytes(releases: &[UpstreamRelease]) -> Vec<u8> {
    serde_json::to_vec(&releases.iter().map(|release| serde_json::json!({
        "tag_name": release.tag_name, "draft": release.draft, "prerelease": release.prerelease,
        "assets": release.assets.iter().map(|asset| serde_json::json!({
            "name": asset.name, "browser_download_url": asset.browser_download_url, "size": asset.size, "digest": asset.digest,
        })).collect::<Vec<_>>(),
    })).collect::<Vec<_>>()).unwrap()
}

#[test]
fn fake_provider_catalog_preselects_stable_and_loads_older_releases() {
    for (provider, newest, older) in [
        (Provider::GeProton, "GE-Proton10-20", "GE-Proton9-20"),
        (Provider::ProtonCachyos, "250101", "241101"),
        (Provider::Dxvk, "v2.6", "v2.5"),
        (Provider::Vkd3d, "v2.14", "v2.13"),
    ] {
        let (_root, mut app) = fixture();
        open(&mut app);
        send(Event::Choose, &mut app);
        send(Event::Provider(provider), &mut app);
        let mut pre = upstream(provider, newest);
        pre.prerelease = true;
        let mut first = vec![pre, upstream(provider, newest)];
        first.extend((0..28).map(|_| UpstreamRelease {
            tag_name: "draft".into(),
            draft: true,
            prerelease: false,
            assets: Vec::new(),
        }));
        let fake = Client::with_transport(Fake(BTreeMap::from([
            (catalog_url(provider, 1), page_bytes(&first)),
            (
                catalog_url(provider, 2),
                page_bytes(&[upstream(provider, older)]),
            ),
        ])));
        let result = downloads_work::list(&fake, provider, 1);
        send(
            Event::Listed(app.downloads_page.release_generation, 1, result),
            &mut app,
        );
        assert_eq!(app.downloads_page.selected.as_deref(), Some(newest));
        assert_eq!(app.downloads_page.releases.len(), 1);
        assert!(app.downloads_page.has_more);
        send(Event::More, &mut app);
        assert!(app.downloads_page.listing);
        let result = downloads_work::list(&fake, provider, 2);
        send(
            Event::Listed(app.downloads_page.release_generation, 2, result),
            &mut app,
        );
        assert_eq!(app.downloads_page.releases.len(), 2);
        assert!(!app.downloads_page.has_more);
        assert_eq!(app.downloads_page.selected.as_deref(), Some(newest));
        send(Event::Select(older.into()), &mut app);
        assert_eq!(app.downloads_page.selected.as_deref(), Some(older));
        let _ = app.downloads_view();
    }
}

#[test]
fn unsupported_first_page_is_skipped_and_provider_changes_ignore_stale_catalogs() {
    let (_root, mut app) = fixture();
    open(&mut app);
    send(Event::Choose, &mut app);
    send(Event::Provider(Provider::Dxvk), &mut app);
    let stale = app.downloads_page.release_generation;
    send(Event::Provider(Provider::GeProton), &mut app);
    send(Event::Listed(stale, 1, Err("late".into())), &mut app);
    assert!(app.downloads_page.listing);
    assert!(app.downloads_page.error.is_none());
    let generation = app.downloads_page.release_generation;
    send(
        Event::Listed(
            generation,
            1,
            Ok(ReleasePage {
                releases: Vec::new(),
                has_more: true,
            }),
        ),
        &mut app,
    );
    assert!(app.downloads_page.listing);
    assert_eq!(app.downloads_page.page, 1);
    let release = select_asset(
        Provider::GeProton,
        upstream(Provider::GeProton, "GE-Proton9-20"),
    )
    .unwrap()
    .unwrap();
    send(
        Event::Listed(
            generation,
            2,
            Ok(ReleasePage {
                releases: vec![release],
                has_more: false,
            }),
        ),
        &mut app,
    );
    assert_eq!(
        app.downloads_page.selected.as_deref(),
        Some("GE-Proton9-20")
    );
    assert!(!app.downloads_page.listing);
}

fn install_fixture() -> (Client<Fake>, Release) {
    let mut tar = tar::Builder::new(Vec::new());
    for arch in ["x64", "x32"] {
        let bytes = b"fake dll";
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, format!("dxvk-2.6/{arch}/dxgi.dll"), &bytes[..])
            .unwrap();
    }
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(&tar.into_inner().unwrap()).unwrap();
    let bytes = gzip.finish().unwrap();
    let mut upstream = upstream(Provider::Dxvk, "v2.6");
    upstream.assets[0].size = bytes.len() as u64;
    upstream.assets[0].digest = Some(format!("sha256:{:x}", Sha256::digest(&bytes)));
    let url = upstream.assets[0].browser_download_url.clone();
    (
        Client::with_transport(Fake(BTreeMap::from([(url, bytes)]))),
        select_asset(Provider::Dxvk, upstream).unwrap().unwrap(),
    )
}

fn begin_install(app: &mut App, release: &Release) -> (u64, Token) {
    open(app);
    send(Event::Choose, app);
    send(Event::Provider(release.provider), app);
    send(
        Event::Listed(
            app.downloads_page.release_generation,
            1,
            Ok(ReleasePage {
                releases: vec![release.clone()],
                has_more: false,
            }),
        ),
        app,
    );
    send(Event::Install, app);
    let Some(removal::Change::Downloads(Change::Download { token, .. })) = &app.change else {
        panic!("download did not start")
    };
    (app.change_generation, token.clone())
}

#[test]
fn fake_install_reports_progress_and_cancel_before_publication_leaves_nothing() {
    let (_root, mut app) = fixture();
    let (client, release) = install_fixture();
    let (generation, token) = begin_install(&mut app, &release);
    let paths = app.paths.clone();
    let result = downloads_work::install(&client, &paths, &release, &token, |line| {
        send(Event::Progress(generation, line), &mut app);
        send(Event::Cancel, &mut app);
    });
    assert!(token.is_canceled());
    assert!(
        app.downloads_page
            .status
            .as_ref()
            .unwrap()
            .contains("Canceling")
    );
    assert!(result.as_ref().unwrap().is_none());
    assert!(!paths.default_dxvk_root().join("dxvk-2.6").exists());
    assert!(
        !fs::read_dir(paths.default_dxvk_root())
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".wineo-stage"))
    );
    let page_generation = app.downloads_page.generation;
    send(Event::Downloaded(generation, result), &mut app);
    assert_eq!(
        app.downloads_page.generation,
        page_generation.wrapping_add(1)
    );
    assert!(app.change.is_none());
    assert!(app.downloads_page.loading);
    load(&mut app);
    assert!(app.downloads_page.snapshot().unwrap().builds.is_empty());
    assert_eq!(
        app.downloads_page.notice.as_deref(),
        Some("Download canceled.")
    );
}

#[test]
fn fake_install_keeps_published_build_when_canceled_and_refreshes() {
    let (_root, mut app) = fixture();
    let (client, release) = install_fixture();
    let (generation, token) = begin_install(&mut app, &release);
    let paths = app.paths.clone();
    let mut progress = Vec::new();
    let result = downloads_work::install(&client, &paths, &release, &token, |line| {
        progress.push(line.clone());
        let published = line.starts_with("Installed in");
        send(Event::Progress(generation, line), &mut app);
        if published {
            send(Event::Cancel, &mut app);
        }
    });
    assert!(token.is_canceled());
    assert!(result.as_ref().unwrap().is_some());
    assert!(
        paths
            .default_dxvk_root()
            .join("dxvk-2.6/.wineo-install.json")
            .is_file()
    );
    assert!(
        progress
            .iter()
            .any(|line| line.starts_with("Downloading dxvk"))
    );
    assert!(progress.iter().any(|line| line == "Verifying dxvk"));
    assert!(progress.iter().any(|line| line == "Extracting dxvk"));
    let page_generation = app.downloads_page.generation;
    send(Event::Downloaded(generation, result), &mut app);
    assert_eq!(
        app.downloads_page.generation,
        page_generation.wrapping_add(1)
    );
    load(&mut app);
    assert_eq!(
        build(&app, Kind::Dxvk, "dxvk-2.6").users,
        ["template gaming (latest)"]
    );
}

fn nodes(tree: &iced::advanced::widget::Tree) -> usize {
    1 + tree.children.iter().map(nodes).sum::<usize>()
}

#[test]
fn running_changes_survive_sections_show_sidebar_spinner_and_close_waits() {
    for downloading in [false, true] {
        let (_root, mut app) = fixture();
        runner(&app.paths, "unused", false);
        open(&mut app);
        let idle_sidebar = nodes(&iced::advanced::widget::Tree::new(
            app.sidebar().as_widget(),
        ));
        let token = if downloading {
            let (_, release) = install_fixture();
            let (_, token) = begin_install(&mut app, &release);
            Some(token)
        } else {
            let unused = build(&app, Kind::Runner, "unused");
            review(&mut app, &unused);
            send(Event::Confirm, &mut app);
            None
        };
        assert!(app.spinning() && app.foreground_busy());
        assert_eq!(
            nodes(&iced::advanced::widget::Tree::new(
                app.sidebar().as_widget()
            )),
            idle_sidebar + 1
        );
        let generation = app.change_generation;
        let _ = app.update(Message::Section(sidebar::Section::Library));
        assert_eq!(app.section, sidebar::Section::Library);
        assert!(app.change.is_some());
        if let Some(token) = &token {
            assert!(!token.is_canceled());
        }
        let _ = app.update(Message::Window(iced::window::Event::CloseRequested));
        assert!(app.close_prompt && !app.closing);
        let _ = app.update(Message::CancelAndClose);
        assert!(app.closing && app.foreground_busy());
        if let Some(token) = &token {
            assert!(token.is_canceled());
            send(
                Event::Downloaded(generation.wrapping_sub(1), Ok(None)),
                &mut app,
            );
            assert!(app.change.is_some());
            send(Event::Downloaded(generation, Ok(None)), &mut app);
        } else {
            send(
                Event::Deleted(generation.wrapping_sub(1), Ok((0, Vec::new()))),
                &mut app,
            );
            assert!(app.change.is_some());
            send(Event::Deleted(generation, Ok((0, Vec::new()))), &mut app);
        }
        assert!(app.change.is_none());
        assert!(!app.foreground_busy());
    }
}

#[test]
fn failed_reads_and_release_errors_can_retry_without_creating_changes() {
    let (_root, mut app) = fixture();
    let _ = app.update(Message::Section(sidebar::Section::Downloads));
    send(
        Event::Loaded(app.downloads_page.generation, Err("unreadable root".into())),
        &mut app,
    );
    send(Event::Choose, &mut app);
    assert!(!app.downloads_page.choosing);
    send(Event::Refresh, &mut app);
    assert!(app.downloads_page.loading);
    load(&mut app);
    send(Event::Choose, &mut app);
    send(Event::Provider(Provider::Dxvk), &mut app);
    send(
        Event::Listed(
            app.downloads_page.release_generation,
            1,
            Err("offline".into()),
        ),
        &mut app,
    );
    assert_eq!(app.downloads_page.error.as_deref(), Some("offline"));
    assert!(app.change.is_none());
    send(Event::Provider(Provider::Dxvk), &mut app);
    assert!(app.downloads_page.listing);
    assert!(app.downloads_page.error.is_none());
}

#[test]
fn other_launchers_builds_are_listed_apart_and_never_deletable() {
    let (root, mut app) = fixture();
    runner(&app.paths, "wine-tkg", false);
    let steam = crate::runner_label::fake_steam_build(root.path(), "GE-Proton11-3");
    let unusable = root.path().join(".steam/root/compatibilitytools.d/Broken");
    fs::create_dir_all(&unusable).unwrap();
    let global = app.paths.global_config();
    fs::create_dir_all(global.parent().unwrap()).unwrap();
    fs::write(&global, "external_runners = true\n").unwrap();
    config(&app.paths, "celeste", "runner = 'GE-Proton11-3'\n");
    open(&mut app);

    let own = build(&app, Kind::Runner, "wine-tkg");
    assert_eq!(own.launcher, None);
    assert!(own.deletable());
    let external = build(&app, Kind::Runner, "GE-Proton11-3");
    assert_eq!(external.launcher, Some(Source::Steam));
    assert!(external.users.iter().any(|user| user == "celeste"));
    assert!(!external.deletable());
    assert!(external.detail().starts_with("Steam, used by celeste"));
    assert!(
        app.downloads_page
            .snapshot()
            .unwrap()
            .skipped
            .iter()
            .any(|line| line.starts_with("Steam / Broken: "))
    );

    // Unused, it's still not wineo's to delete.
    fs::remove_file(app.paths.environment_config("celeste")).unwrap();
    load(&mut app);
    let external = build(&app, Kind::Runner, "GE-Proton11-3");
    assert!(!external.deletable());
    send(Event::Review(external.id()), &mut app);
    assert!(!app.downloads_confirm_open());
    assert_eq!(
        downloads_work::remove(&app.paths, &external).unwrap_err(),
        "GE-Proton11-3 belongs to Steam."
    );
    assert!(downloads_work::size(&app.paths, &external).is_err());
    assert!(steam.join("proton").exists());
    let _ = app.downloads_view();
}

#[test]
fn a_different_build_behind_a_name_is_noted_on_its_row() {
    let mut build = Build::new(Kind::Runner, "GE-Proton-latest");
    assert_eq!(build.detail(), "unused");
    build.users = vec!["dusk".into()];
    build.different = vec![Source::Heroic];
    assert_eq!(build.detail(), "used by dusk, Heroic has a different build");
    build.launcher = Some(Source::Steam);
    build.different = vec![Source::Heroic, Source::Lutris];
    assert_eq!(
        build.detail(),
        "Steam, used by dusk, Heroic, Lutris have different builds"
    );
}
