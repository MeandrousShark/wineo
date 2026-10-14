//! Galaxy installs against the fake content system.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::paths::Paths;

type Hook = Box<dyn FnMut(&Paths)>;

thread_local! {
    static AFTER_STORE: RefCell<Option<Hook>> = const { RefCell::new(None) };
    static FREE_AT: RefCell<Vec<(PathBuf, u64)>> = const { RefCell::new(Vec::new()) };
    static DEVICE_AT: RefCell<Vec<(PathBuf, u64)>> = const { RefCell::new(Vec::new()) };
}

/// Runs a test's hook once the store versions are held and downloaded.
pub(super) fn after_store(paths: &Paths) {
    if let Some(mut hook) = AFTER_STORE.with(|hook| hook.borrow_mut().take()) {
        hook(paths);
    }
}

fn longest(list: &[(PathBuf, u64)], path: &Path) -> Option<u64> {
    list.iter()
        .filter(|(prefix, _)| path.starts_with(prefix))
        .max_by_key(|(prefix, _)| prefix.components().count())
        .map(|(_, value)| *value)
}

/// The free space a test says the filesystem of `path` has.
pub(super) fn free_at(path: &Path) -> Option<u64> {
    FREE_AT.with(|list| longest(&list.borrow(), path))
}

/// The filesystem a test says `path` is on.
pub(super) fn device_at(path: &Path) -> Option<u64> {
    DEVICE_AT.with(|list| longest(&list.borrow(), path))
}

use std::collections::HashSet;
use std::fs;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::json as value;
use tempfile::TempDir;
use wineo_gog::fake::content::{
    Cdn, ContentSystem, FakeBuild, FakeDependency, FakeDepot, FakeFile,
};
use wineo_gog::fake::{self, FakeGog, Reply};

use super::install::{self, Plan};
use super::record::{self, Record};
use super::*;
use crate::cancel::Token;
use crate::config::{Arch, GlobalConfig};
use crate::environment::Environment;
use crate::gog::account::{self, Session};
use crate::gog::folder::{
    self, FolderLock, FolderState, JournalKind, ProductStatus, set_failpoint, set_failpoint_nth,
};
use crate::gog::test_support::{FakeKeyring, client_at, setup, tokens};
use crate::gog::update::install::{Installed, Request};
use crate::test_script::write_script;

const GAME: u64 = 1207658924;
const DLC: u64 = 1207658925;
const OTHER: u64 = 1207658930;
const USER: u64 = 1001;

/// What the fake GOG says besides the content system.
#[derive(Default)]
struct Gog {
    /// Owned products. None owns the game and its DLC.
    owned: Option<Vec<u64>>,
    /// DLC listed in the game's details.
    dlc: Vec<u64>,
    /// Builds left out of build lists, as GOG drops old ones.
    hidden: HashSet<String>,
    /// Revokes every secure link once this many chunks were served.
    revoke_after: Option<usize>,
    chunks: usize,
}

struct Fixture {
    _root: TempDir,
    paths: Paths,
    keyring: Arc<FakeKeyring>,
    content: Arc<ContentSystem>,
    gog: Arc<Mutex<Gog>>,
    server: FakeGog,
    folder: PathBuf,
}

fn info(id: u64, build: &str) -> String {
    value!({
        "gameId": id.to_string(),
        "rootGameId": GAME.to_string(),
        "buildId": build,
        "name": "Fake Game",
        "language": "English",
        "languages": ["en-US"],
        "playTasks": [{
            "type": "FileTask",
            "isPrimary": true,
            "path": "bin\\Game.exe",
            "workingDir": "bin",
        }],
        "version": 1,
    })
    .to_string()
}

impl Fixture {
    fn new() -> Self {
        let (root, paths) = setup();
        let keyring = Arc::new(FakeKeyring::default());
        let secret = format!(r#"{{"refresh_token":"R","user_id":"{USER}"}}"#);
        crate::key_store::save_key(&account::KEY_SPEC, &paths, &*keyring, &secret).unwrap();
        // Small chunks, so most files come in several.
        let content = Arc::new(ContentSystem::new().with_chunk_size(8));
        let gog = Arc::new(Mutex::new(Gog::default()));
        let server = {
            let (content, gog) = (Arc::clone(&content), Arc::clone(&gog));
            FakeGog::start(move |request| answer(&content, &gog, request))
        };
        let folder = fs::canonicalize(root.path())
            .unwrap()
            .join("Games")
            .join("Fake Game");
        let bin = GlobalConfig::default()
            .runner_root(&paths)
            .unwrap()
            .join("fake/bin");
        fs::create_dir_all(&bin).unwrap();
        write_script(bin.join("wineserver"), "#!/bin/sh\nexit 0\n");
        write_script(
            bin.join("wine"),
            "#!/bin/sh\ncase \"$1\" in\nwineboot) [ \"$2\" = --init ] && echo 'WINE REGISTRY Version 2' > \"$WINEPREFIX/system.reg\" ;;\nesac\nexit 0\n",
        );
        Self {
            _root: root,
            paths,
            keyring,
            content,
            gog,
            server,
            folder,
        }
    }

    /// The game with a few files and its info file, as build `build`.
    fn game(build: &str) -> FakeBuild {
        FakeBuild::new(GAME, build).depot(
            FakeDepot::new(GAME)
                .file(FakeFile::new("bin\\Game.exe", b"MZ the game itself").flag("executable"))
                .file(FakeFile::new("Data\\core.dat", b"core data, several chunks long").with_md5())
                .file(FakeFile::new(
                    "Data\\big.dat",
                    b"many chunks and no whole-file md5",
                ))
                .file(FakeFile::new("empty.txt", b""))
                .file(FakeFile::new(
                    &format!("goggame-{GAME}.info"),
                    info(GAME, build).as_bytes(),
                ))
                .directory("saves")
                .link("Data\\current", "core.dat"),
        )
    }

    fn session(&self) -> Session<'_> {
        Session::open(&self.paths, &*self.keyring, client_at(&self.server.base))
            .unwrap()
            .unwrap()
    }

    fn request(&self) -> Request {
        Request {
            name: "game".to_owned(),
            gog_id: GAME,
            dir: Some(self.folder.clone()),
            refresh: true,
            runner: Some("fake".to_owned()),
            ..Request::default()
        }
    }

    fn plan_with(&self, request: &Request) -> Result<Plan> {
        install::plan(&self.paths, &mut self.session(), request, &Token::new())
    }

    fn plan(&self) -> Plan {
        self.plan_with(&self.request()).unwrap()
    }

    fn apply(&self, plan: &Plan) -> Result<Installed> {
        install::apply(
            &self.paths,
            &mut self.session(),
            plan,
            &Token::new(),
            |_| {},
        )
    }

    fn install(&self) -> Installed {
        self.apply(&self.plan()).unwrap()
    }

    fn state(&self) -> FolderState {
        folder::peek(&self.paths, &self.folder).unwrap().unwrap()
    }

    fn record(&self) -> Option<Record> {
        let bytes = fs::read(folder::sidecar_file(&self.paths, &self.folder)).ok()?;
        Record::parse(&bytes)
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.folder.join(path)).unwrap()
    }

    /// Chunk requests so far, by the CDN they went to.
    fn chunk_requests(&self) -> Vec<Cdn> {
        self.server
            .requests()
            .iter()
            .filter_map(ContentSystem::cdn_of)
            .collect()
    }

    fn secure_links(&self) -> usize {
        self.server
            .requests()
            .iter()
            .filter(|request| {
                request.path().ends_with("/secure_link") || request.path() == "/open_link"
            })
            .count()
    }

    fn hide(&self, build: &str) {
        self.gog.lock().unwrap().hidden.insert(build.to_owned());
    }

    fn support(&self, product: u64, path: &str) -> PathBuf {
        self.paths
            .gog_support_dir(&folder::key(&self.folder))
            .join(product.to_string())
            .join(path)
    }

    /// Every entry left in the game folder, `.wineo-` temporaries included.
    fn tree(&self) -> Vec<String> {
        let mut found = Vec::new();
        walk(&self.folder, "", &mut found);
        found.sort();
        found
    }
}

fn walk(dir: &Path, prefix: &str, found: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let entry = entry.unwrap();
        let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
        let kind = entry.file_type().unwrap();
        found.push(name.clone());
        if kind.is_dir() {
            walk(&entry.path(), &format!("{name}/"), found);
        }
    }
}

fn answer(content: &ContentSystem, gog: &Mutex<Gog>, request: &fake::Request) -> Reply {
    let path = request.path().to_owned();
    let mut gog = gog.lock().unwrap();
    if ContentSystem::cdn_of(request).is_some() {
        gog.chunks += 1;
        if gog.revoke_after == Some(gog.chunks) {
            content.revoke_links(403);
        }
    }
    if path == "/token" {
        return Reply::Send(tokens("ACCESS", "R", USER, 3600));
    }
    if path == "/user/data/games" {
        let owned = gog.owned.clone().unwrap_or_else(|| {
            std::iter::once(GAME)
                .chain(gog.dlc.iter().copied())
                .collect()
        });
        return fake::json(200, &value!({ "owned": owned }).to_string());
    }
    if let Some(id) = path.strip_prefix("/products/")
        && let Ok(id) = id.parse::<u64>()
    {
        let title = |id: u64| match id {
            GAME => "Fake Game".to_owned(),
            OTHER => "Other Game".to_owned(),
            id => format!("Fake Game - DLC {id}"),
        };
        let mut details = value!({
            "id": id,
            "title": title(id),
            "downloads": {"installers": [], "patches": []},
        });
        if id == GAME {
            details["dlcs"] = value!({
                "products": gog.dlc.iter().map(|id| value!({"id": id})).collect::<Vec<_>>(),
            });
            details["expanded_dlcs"] = gog
                .dlc
                .iter()
                .map(|id| value!({"id": id, "title": title(*id)}))
                .collect();
        }
        return fake::json(200, &details.to_string());
    }
    let answered = content.answer(request);
    if path.ends_with("/os/windows/builds")
        && let Some(Reply::Send(bytes)) = &answered
        && !gog.hidden.is_empty()
    {
        let text = String::from_utf8_lossy(bytes);
        let body = &text[text.find("\r\n\r\n").unwrap() + 4..];
        let mut list: serde_json::Value = serde_json::from_str(body).unwrap();
        list["items"].as_array_mut().unwrap().retain(|item| {
            !gog.hidden
                .contains(item["build_id"].as_str().unwrap_or_default())
        });
        return fake::json(200, &list.to_string());
    }
    answered.unwrap_or_else(|| fake::json(404, "{}"))
}

/// The game, installed and committed, with every file in place.
fn assert_installed(fx: &Fixture) {
    let state = fx.state();
    assert_eq!(state.journal, None);
    let committed = state.committed.as_ref().unwrap();
    assert_eq!(committed.track, Track::Galaxy);
    assert_eq!(committed.platform, Platform::Windows);
    assert_eq!(committed.source, Some(folder::Source::Install));
    assert_eq!(fx.read("bin/Game.exe"), "MZ the game itself");
    assert_eq!(fx.read("Data/core.dat"), "core data, several chunks long");
    assert_eq!(fx.read("Data/big.dat"), "many chunks and no whole-file md5");
    assert_eq!(fx.read("empty.txt"), "");
    assert!(fx.folder.join("saves").is_dir());
    assert_eq!(
        fs::read_link(fx.folder.join("Data/current")).unwrap(),
        Path::new("core.dat")
    );
    assert!(
        !fx.tree()
            .iter()
            .any(|path| path.rsplit('/').next().unwrap().starts_with(".wineo-")),
        "{:?}",
        fx.tree()
    );
    // The record holds the installed manifest, with every file's MD5, and
    // nothing pending.
    let record = fx.record().unwrap();
    assert!(record.pending.is_none());
    let installed = record::installed(&state, &record).unwrap().unwrap();
    assert!(
        installed
            .manifest
            .files
            .iter()
            .all(|file| file.md5.is_some())
    );
}

#[test]
fn a_galaxy_install_downloads_every_file_and_commits() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    let plan = fx.plan();
    assert_eq!(plan.build(), "100");
    assert_eq!(plan.products().len(), 1);
    assert!(!fx.folder.exists());

    let installed = fx.install();
    assert_installed(&fx);
    assert_eq!(installed.target, Some(fx.folder.join("bin/Game.exe")));
    let environment = Environment::load(&fx.paths, "game").unwrap();
    assert_eq!(
        environment.config.gog_id.as_deref(),
        Some(GAME.to_string().as_str())
    );
    let state = fx.state();
    let base = &state.committed.unwrap().products[0];
    assert_eq!(base.build.as_deref(), Some("100"));
    // Executable flags are ignored.
    let mode = fs::metadata(fx.folder.join("bin/Game.exe"))
        .unwrap()
        .permissions();
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
        0o644
    );
}

/// The chunks of the plan's files not already in the folder whole.
fn chunks_left(fx: &Fixture, plan: &Plan) -> usize {
    plan.manifest_for_tests()
        .files
        .iter()
        .filter(|file| {
            let there = fs::read(fx.folder.join(&file.path)).ok();
            there.is_none_or(|bytes| {
                file.md5.as_deref() != Some(fake::content::md5_hex(&bytes).as_str())
            })
        })
        .map(|file| file.chunks.len())
        .sum::<usize>()
        + plan
            .manifest_for_tests()
            .support
            .iter()
            .map(|file| file.chunks.len())
            .sum::<usize>()
}

fn chunks_in(plan: &Plan) -> usize {
    plan.manifest_for_tests()
        .files
        .iter()
        .chain(&plan.manifest_for_tests().support)
        .map(|file| file.chunks.len())
        .sum()
}

/// An install stopped by an injected failure at `point`.
fn interrupted(fx: &Fixture, point: &'static str, skip: usize) -> anyhow::Error {
    set_failpoint_nth(point, skip);
    let result = fx.plan_with(&fx.request()).and_then(|plan| fx.apply(&plan));
    set_failpoint(None);
    let error = result.unwrap_err();
    assert!(
        format!("{error:#}").contains("injected"),
        "{point}: {error:#}"
    );
    error
}

#[test]
fn an_install_interrupted_at_each_step_is_finished_by_installing_again() {
    for (point, skip) in [
        ("galaxy.create.before", 0),
        ("galaxy.pending.after", 0),
        ("galaxy.journal.after", 0),
        ("galaxy.environment.after", 0),
        ("galaxy.placed.after", 0),
        ("galaxy.file.after", 1),
        ("galaxy.files.after", 0),
        ("galaxy.installed.after", 0),
    ] {
        let fx = Fixture::new();
        fx.content.add_build(Fixture::game("100"));
        interrupted(&fx, point, skip);
        let journaled = folder::peek(&fx.paths, &fx.folder).unwrap();
        if let Some(state) = &journaled {
            assert!(state.journal.is_some(), "{point}");
            // A journal blocks launches until the install is finished.
            assert!(
                folder::precheck(
                    &fx.paths,
                    "game",
                    Some(&GAME.to_string()),
                    &[fx.folder.join("bin/Game.exe")]
                )
                .is_err(),
                "{point}"
            );
        }
        let plan = fx.plan();
        assert_eq!(plan.resumes(), journaled.is_some(), "{point}");
        fx.apply(&plan).unwrap();
        assert_installed(&fx);
    }
}

#[test]
fn finishing_an_install_keeps_the_files_already_in_place() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    let total = chunks_in(&fx.plan());
    // Three files done, so at least one with chunks was put in place.
    interrupted(&fx, "galaxy.file.after", 2);
    let before = fx.chunk_requests().len();
    let left = chunks_left(&fx, &fx.plan());
    assert!(left < total, "{left} of {total}");
    // Pending belongs to the journal, so it is the target.
    let state = fx.state();
    let record = fx.record().unwrap();
    assert!(record::pending(&state, &record).is_some());
    assert!(record::installed(&state, &record).unwrap().is_none());

    fx.install();
    assert_installed(&fx);
    assert_eq!(fx.chunk_requests().len() - before, left);
}

#[test]
fn a_crash_after_the_commit_leaves_installed_matching_and_pending_ignored() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    interrupted(&fx, "galaxy.commit.after", 0);
    let state = fx.state();
    assert!(state.journal.is_none());
    let record = fx.record().unwrap();
    assert!(record.pending.is_some());
    // Nothing is promoted from pending, and none is needed: installed was
    // written before the commit.
    assert!(record::pending(&state, &record).is_none());
    assert!(record::installed(&state, &record).unwrap().is_some());
}

#[test]
fn a_crash_between_installed_and_the_commit_finishes_from_pending() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    interrupted(&fx, "galaxy.installed.after", 0);
    let state = fx.state();
    let record = fx.record().unwrap();
    // Nothing committed yet, so installed doesn't count.
    assert!(record::installed(&state, &record).unwrap().is_none());
    assert!(record::pending(&state, &record).is_some());
    let before = fx.chunk_requests().len();
    fx.install();
    assert_installed(&fx);
    assert_eq!(fx.chunk_requests().len(), before, "nothing left to fetch");
}

#[test]
fn a_chunk_failing_its_hash_fails_the_install_and_names_no_link() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    let hash = fake::content::md5_hex(&fake::content::zlib(b"MZ the g"));
    fx.content.corrupt_chunk(&hash);
    let error = fx.apply(&fx.plan()).unwrap_err();
    let shown = format!("{error:#} {error:?}");
    assert!(shown.contains("bin/Game.exe"), "{shown}");
    for secret in ["FAKETOKEN", "wsSecret", "token="] {
        assert!(!shown.contains(secret), "{shown}");
    }
    // Both CDNs were tried.
    let cdns = fx.chunk_requests();
    assert!(cdns.contains(&Cdn::Primary) && cdns.contains(&Cdn::Fallback));
    assert!(!fx.folder.join("bin/Game.exe").exists());
    assert!(fx.state().journal.is_some());
}

#[test]
fn an_expired_link_is_fetched_again_mid_download() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    fx.gog.lock().unwrap().revoke_after = Some(3);
    fx.install();
    assert_installed(&fx);
    assert!(fx.secure_links() >= 2, "{}", fx.secure_links());
}

#[test]
fn the_fallback_cdn_is_used_when_the_first_fails() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    let hash = fake::content::md5_hex(&fake::content::zlib(b"MZ the g"));
    fx.content.missing_on_primary(&hash);
    fx.install();
    assert_installed(&fx);
    assert!(fx.chunk_requests().contains(&Cdn::Fallback));
}

#[test]
fn support_files_go_to_the_support_folder() {
    let fx = Fixture::new();
    fx.content.add_build(
        Fixture::game("100").depot(
            FakeDepot::new(GAME)
                .file(FakeFile::new("__support\\setup.exe", b"setup program").flag("support"))
                .file(FakeFile::new("userdocs\\rules.txt", b"rules").flag("support")),
        ),
    );
    let plan = fx.plan();
    assert_eq!(plan.support_files(), 2);
    fx.install();
    assert_installed(&fx);
    assert_eq!(
        fs::read_to_string(fx.support(GAME, "__support/setup.exe")).unwrap(),
        "setup program"
    );
    assert_eq!(
        fs::read_to_string(fx.support(GAME, "userdocs/rules.txt")).unwrap(),
        "rules"
    );
    assert!(!fx.folder.join("__support").exists());
    assert!(!fx.folder.join("userdocs").exists());
    let record = fx.record().unwrap();
    let support = &record.installed.unwrap().manifest.support;
    assert!(support.iter().all(|file| file.md5.is_some()));

    // The support folder goes with the state.
    folder::release(&fx.paths, &fx.folder, &|| false).unwrap();
    assert!(folder::forget(&fx.paths, &fx.folder, &|| false).unwrap());
    assert!(!fx.support(GAME, "").exists());
    assert!(fx.record().is_none());
}

#[test]
fn depots_are_filtered_by_the_prefix_arch() {
    for (arch, kept, dropped) in [
        (Arch::Win64, "lib64.dll", "lib32.dll"),
        (Arch::Win32, "lib32.dll", "lib64.dll"),
    ] {
        let fx = Fixture::new();
        fx.content.add_build(
            Fixture::game("100")
                .depot(
                    FakeDepot::new(GAME)
                        .bitness(&["32"])
                        .file(FakeFile::new("lib32.dll", b"32-bit")),
                )
                .depot(
                    FakeDepot::new(GAME)
                        .bitness(&["64"])
                        .file(FakeFile::new("lib64.dll", b"64-bit")),
                ),
        );
        let request = Request {
            arch: Some(arch),
            ..fx.request()
        };
        let plan = fx.plan_with(&request).unwrap();
        assert_eq!(plan.arch(), arch);
        fx.apply(&plan).unwrap();
        assert!(fx.folder.join(kept).is_file(), "{arch:?}");
        assert!(!fx.folder.join(dropped).exists(), "{arch:?}");
    }
}

/// A game listing a redistributable for the store and one that's part of
/// the game, with no program.
fn with_dependencies(fx: &Fixture, version: &[u8]) {
    fx.content.set_dependencies(
        "1",
        &[
            FakeDependency::new(
                "MSVC2017",
                "__redist\\MSVC2017\\VC_redist.x86.exe",
                "/quiet",
                FakeDepot::new(0).file(FakeFile::new(
                    "__redist\\MSVC2017\\VC_redist.x86.exe",
                    version,
                )),
            ),
            FakeDependency::new(
                "DOSBOX",
                "",
                "",
                FakeDepot::new(0).file(FakeFile::new("DOSBOX\\dosbox.exe", b"dosbox")),
            ),
        ],
    );
}

#[test]
fn game_local_dependencies_join_the_folder_and_others_the_store() {
    let fx = Fixture::new();
    fx.content.add_build(
        Fixture::game("100")
            .dependency("MSVC2017")
            .dependency("DOSBOX")
            .dependency("NOPE"),
    );
    with_dependencies(&fx, b"runtime v1");
    let plan = fx.plan();
    assert_eq!(plan.redistributables(), ["MSVC2017"]);
    assert!(
        plan.warnings()
            .iter()
            .any(|warning| warning.contains("NOPE"))
    );
    fx.install();
    assert_installed(&fx);
    assert_eq!(fx.read("DOSBOX/dosbox.exe"), "dosbox");
    assert!(!fx.folder.join("__redist").exists());
    let record = fx.record().unwrap();
    let manifest = record.installed.unwrap().manifest;
    let dosbox = manifest
        .files
        .iter()
        .find(|file| file.path == "DOSBOX/dosbox.exe")
        .unwrap();
    assert_eq!(dosbox.dependency.as_deref(), Some("DOSBOX"));
    assert_eq!(manifest.setup.redist.len(), 1);
    let redist = &manifest.setup.redist[0];
    assert_eq!(redist.arguments, "/quiet");
    let stored = fx
        .paths
        .gog_redist_dir()
        .join("MSVC2017")
        .join(&redist.manifest)
        .join("__redist/MSVC2017/VC_redist.x86.exe");
    assert_eq!(fs::read_to_string(stored).unwrap(), "runtime v1");
}

#[test]
fn an_unstarted_operation_is_never_taken_for_tracked() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    fx.install();
    // A repair or reinstall of this very selection wrote pending and
    // stopped before its journal, with no installed record and a file that
    // doesn't match.
    let state = fx.state();
    let mut record = fx.record().unwrap();
    let mut pending = record.pending.clone().unwrap_or_else(|| record::Pending {
        operation: "20261014T000000-1".to_owned(),
        selection: record::digest(state.committed.as_ref().unwrap()),
        manifest: record.installed.clone().unwrap().manifest,
        store: Vec::new(),
    });
    pending.selection = record::digest(state.committed.as_ref().unwrap());
    record.pending = Some(pending);
    record.installed = None;
    write_record(&fx, &record);
    fs::write(fx.folder.join("Data/core.dat"), "yours").unwrap();
    let record = fx.record().unwrap();
    assert!(record::pending(&state, &record).is_none());
    assert!(record::installed(&state, &record).unwrap().is_none());

    // Nor one whose journal is another operation's.
    let mut journaled = state.clone();
    journaled.journal = Some(folder::Journal {
        operation: "20261014T000000-2".to_owned(),
        kind: JournalKind::Reinstall,
        target: crate::gog::folder::Target {
            gog_id: GAME,
            platform: Platform::Windows,
            language: state.committed.as_ref().unwrap().language.clone(),
            track: Track::Galaxy,
            products: state.committed.as_ref().unwrap().products.clone(),
        },
        packages: Vec::new(),
        attempted: Vec::new(),
    });
    assert!(record::pending(&journaled, &record).is_none());
}

fn write_record(fx: &Fixture, record: &Record) {
    let lock = FolderLock::exclusive(&fx.paths, &fx.folder, &|| false).unwrap();
    record::write(&lock, record).unwrap();
}

#[test]
fn a_missing_corrupt_or_stale_record_means_no_stored_manifest() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    fx.install();
    let state = fx.state();
    let good = fx.record().unwrap();
    assert!(record::installed(&state, &good).unwrap().is_some());

    // Corrupt.
    let path = folder::sidecar_file(&fx.paths, &fx.folder);
    fs::write(&path, b"{ not json").unwrap();
    assert!(fx.record().is_none());
    // Another version.
    fs::write(
        &path,
        good.to_json()
            .unwrap()
            .replace("\"version\": 1", "\"version\": 2"),
    )
    .unwrap();
    assert!(fx.record().is_none());
    // Missing.
    fs::remove_file(&path).unwrap();
    assert!(fx.record().is_none());

    // Stale: a DLC-only change to the selection changes its digest.
    let mut changed = state.clone();
    changed
        .committed
        .as_mut()
        .unwrap()
        .products
        .push(folder::Product {
            id: DLC,
            version: None,
            build: Some("100".to_owned()),
            language: None,
            reported_language: None,
            status: ProductStatus::Installed,
        });
    assert!(record::installed(&changed, &good).unwrap().is_none());
    // So does a retried DLC.
    let mut retried = changed.clone();
    retried.committed.as_mut().unwrap().products[1].status = ProductStatus::Failed;
    assert_ne!(
        record::digest(retried.committed.as_ref().unwrap()),
        record::digest(changed.committed.as_ref().unwrap())
    );
    // And a changed info file.
    fs::write(
        fx.folder.join(format!("goggame-{GAME}.info")),
        info(GAME, "100") + " ",
    )
    .unwrap();
    assert!(record::installed(&state, &good).unwrap().is_none());
}

#[test]
fn a_build_adopted_from_another_tool_drops_the_installed_record() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    fx.content.add_build(Fixture::game("200"));
    fx.install();
    // Another tool moved the game on to build 200.
    fs::write(
        fx.folder.join(format!("goggame-{GAME}.info")),
        info(GAME, "200"),
    )
    .unwrap();
    let stale = fx.record().unwrap();
    assert!(record::installed(&fx.state(), &stale).unwrap().is_none());

    crate::gog::update::check(&mut fx.session(), "game", true).unwrap();
    let state = fx.state();
    assert_eq!(
        state.committed.as_ref().unwrap().products[0]
            .build
            .as_deref(),
        Some("200")
    );
    assert!(fx.record().unwrap().installed.is_none());

    // Back to 100 by hand: the old record would match again, which is safe
    // since deleting needs a content match; here it's simply gone.
    assert!(record::installed(&state, &stale).unwrap().is_none());
}

#[test]
fn without_pending_the_journal_builds_are_fetched_again() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    interrupted(&fx, "galaxy.file.after", 2);
    fs::remove_file(folder::sidecar_file(&fx.paths, &fx.folder)).unwrap();
    let before = fx.chunk_requests().len();
    let plan = fx.plan();
    assert!(plan.retarget().is_none());
    assert_eq!(plan.build(), "100");
    let left = chunks_left(&fx, &plan);
    assert!(left < chunks_in(&plan));
    fx.apply(&plan).unwrap();
    assert_installed(&fx);
    // What was in place and matched is kept, without its record.
    assert_eq!(fx.chunk_requests().len() - before, left);
}

#[test]
fn without_pending_a_build_no_longer_listed_is_retargeted_to_the_latest() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    interrupted(&fx, "galaxy.file.after", 1);
    // A crash that also lost the record, and GOG moved on.
    fs::write(folder::sidecar_file(&fx.paths, &fx.folder), b"garbage").unwrap();
    fx.hide("100");
    fx.content.add_build(
        Fixture::game("200").depot(FakeDepot::new(GAME).file(FakeFile::new("new.txt", b"new"))),
    );
    let plan = fx.plan();
    assert_eq!(
        plan.retarget(),
        Some(&install::Retarget {
            from: "100".to_owned(),
            to: "200".to_owned()
        })
    );
    assert!(
        plan.warnings()
            .iter()
            .any(|warning| warning.contains("200"))
    );
    fx.apply(&plan).unwrap();
    assert_eq!(fx.read("new.txt"), "new");
    let state = fx.state();
    assert_eq!(
        state.committed.unwrap().products[0].build.as_deref(),
        Some("200")
    );
}

#[test]
fn two_installs_use_two_versions_of_one_redistributable_at_once() {
    let fx = Fixture::new();
    fx.content
        .add_build(Fixture::game("100").dependency("MSVC2017"));
    fx.content.add_build(
        FakeBuild::new(OTHER, "300")
            .dependency("MSVC2017")
            .depot(FakeDepot::new(OTHER).file(FakeFile::new("other.exe", b"other"))),
    );
    with_dependencies(&fx, b"runtime v1");
    let first = fx.plan();
    with_dependencies(&fx, b"runtime v2");
    let other = Request {
        name: "other".to_owned(),
        gog_id: OTHER,
        dir: Some(fx.folder.with_file_name("Other Game")),
        ..fx.request()
    };
    let second = fx.plan_with(&other).unwrap();
    let hash = |plan: &Plan| plan.manifest_for_tests().setup.redist[0].manifest.clone();
    assert_ne!(hash(&first), hash(&second));

    // Each waits in the middle, holding its version, until both are there.
    let barrier = Arc::new(std::sync::Barrier::new(2));
    std::thread::scope(|scope| {
        for plan in [&first, &second] {
            let (fx, barrier) = (&fx, Arc::clone(&barrier));
            scope.spawn(move || {
                set_after_store(move |_| {
                    barrier.wait();
                });
                fx.apply(plan).unwrap();
            });
        }
    });
    let dir = fx.paths.gog_redist_dir().join("MSVC2017");
    for (plan, content) in [(&first, "runtime v1"), (&second, "runtime v2")] {
        let stored = dir
            .join(hash(plan))
            .join("__redist/MSVC2017/VC_redist.x86.exe");
        assert_eq!(fs::read_to_string(stored).unwrap(), content);
    }
}

#[test]
fn cleanup_between_a_download_and_its_use_keeps_the_version() {
    let fx = Fixture::new();
    fx.content
        .add_build(Fixture::game("100").dependency("MSVC2017"));
    with_dependencies(&fx, b"runtime v1");
    // A version nothing names or holds, which cleanup does remove.
    let orphan = fx
        .paths
        .gog_redist_dir()
        .join("MSVC2017")
        .join("0".repeat(32));
    fs::create_dir_all(orphan.join("__redist")).unwrap();
    let plan = fx.plan();
    let version = plan.manifest_for_tests().setup.redist[0].manifest.clone();
    let kept = fx.paths.gog_redist_dir().join("MSVC2017").join(&version);
    set_after_store(move |paths| {
        let removed = store::clean(paths).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].manifest, "0".repeat(32));
    });
    fx.apply(&plan).unwrap();
    assert!(kept.is_dir());
    assert!(!orphan.exists());
}

#[test]
fn cleanup_skips_a_held_version_even_when_no_record_names_it() {
    let fx = Fixture::new();
    let version = record::StoreVersion {
        dependency: "DirectX".to_owned(),
        manifest: "a".repeat(32),
    };
    let dir = store::version_dir(&fx.paths, &version);
    fs::create_dir_all(&dir).unwrap();
    let held = store::hold(&fx.paths, &version, &Token::new()).unwrap();
    assert!(held.present());
    assert!(store::clean(&fx.paths).unwrap().is_empty());
    assert!(dir.is_dir());
    drop(held);
    assert_eq!(store::clean(&fx.paths).unwrap(), [version]);
    assert!(!dir.exists());
}

#[test]
fn a_filesystem_short_of_space_refuses_before_anything_changes() {
    let fx = Fixture::new();
    fx.content
        .add_build(Fixture::game("100").depot(
            FakeDepot::new(GAME).file(FakeFile::new("setup.exe", b"setup").flag("support")),
        ));
    let plan = fx.plan();
    // The data directory is on a filesystem of its own, and full.
    let data = fx
        .paths
        .gog_support_dir("x")
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    DEVICE_AT.with(|list| list.borrow_mut().push((data.clone(), 9)));
    FREE_AT.with(|list| list.borrow_mut().push((data, 2)));
    let error = fx.apply(&plan).unwrap_err();
    DEVICE_AT.with(|list| list.borrow_mut().clear());
    FREE_AT.with(|list| list.borrow_mut().clear());
    assert!(error.to_string().starts_with("Not enough space"), "{error}");
    assert!(error.to_string().contains("support"), "{error}");
    assert!(!fx.folder.exists());
    assert!(folder::peek(&fx.paths, &fx.folder).unwrap().is_none());
    assert!(!fx.paths.environment_config("game").exists());
}

#[test]
fn the_track_is_galaxy_when_a_generation_2_build_exists() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    let planned = super::plan(
        &fx.paths,
        &mut fx.session(),
        &fx.request(),
        None,
        &Token::new(),
    )
    .unwrap();
    assert!(matches!(planned, Planned::Galaxy(_)));

    // An explicit installer track is the offline path, which finds no
    // installer here.
    let error = super::plan(
        &fx.paths,
        &mut fx.session(),
        &fx.request(),
        Some(Track::Installer),
        &Token::new(),
    )
    .unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(crate::gog::update::install::InstallError::NoInstaller { .. })
    ));
}

#[test]
fn only_generation_1_builds_go_to_the_installer_or_refuse_galaxy() {
    let fx = Fixture::new();
    let mut old = Fixture::game("100");
    old.generation = 1;
    fx.content.add_build(old);
    let error = super::plan(
        &fx.paths,
        &mut fx.session(),
        &fx.request(),
        None,
        &Token::new(),
    )
    .unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(crate::gog::update::install::InstallError::NoInstaller { .. })
    ));
    let error = super::plan(
        &fx.paths,
        &mut fx.session(),
        &fx.request(),
        Some(Track::Galaxy),
        &Token::new(),
    )
    .unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(GalaxyError::NoBuild { .. })
    ));
    assert_eq!(
        error.to_string(),
        "GOG lists no Galaxy build of Fake Game that wineo can install."
    );
}

#[test]
fn an_offline_install_never_carries_on_a_galaxy_one() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    interrupted(&fx, "galaxy.files.after", 0);
    let error = crate::gog::update::install::plan(
        &fx.paths,
        &mut fx.session(),
        &fx.request(),
        &Token::new(),
    )
    .unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(crate::gog::update::install::InstallError::GalaxyPending { .. })
    ));
    // Picking a track carries it on as it began.
    let planned = super::plan(
        &fx.paths,
        &mut fx.session(),
        &fx.request(),
        Some(Track::Installer),
        &Token::new(),
    )
    .unwrap();
    assert!(matches!(planned, Planned::Galaxy(_)));
}

#[test]
fn a_link_out_of_the_folder_is_refused_while_planning() {
    let fx = Fixture::new();
    fx.content
        .add_build(Fixture::game("100").depot(FakeDepot::new(GAME).link("escape", "..\\..\\etc")));
    let error = fx.plan_with(&fx.request()).unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(GalaxyError::Layout(LayoutError::LinkOutside { .. }))
    ));
}

#[test]
fn an_owned_dlc_installs_with_the_game_and_an_unowned_one_doesnt() {
    let fx = Fixture::new();
    fx.gog.lock().unwrap().dlc = vec![DLC, DLC + 1];
    fx.gog.lock().unwrap().owned = Some(vec![GAME, DLC]);
    fx.content.add_build(
        Fixture::game("100")
            .depot(FakeDepot::new(DLC).file(FakeFile::new("Data\\dlc.dat", b"owned")))
            .depot(FakeDepot::new(DLC + 1).file(FakeFile::new("Data\\nope.dat", b"not owned"))),
    );
    let plan = fx.plan();
    assert_eq!(
        plan.products()
            .iter()
            .map(|product| (product.id, product.status))
            .collect::<Vec<_>>(),
        [
            (GAME, ProductStatus::Installed),
            (DLC, ProductStatus::Installed)
        ]
    );
    fx.install();
    assert_eq!(fx.read("Data/dlc.dat"), "owned");
    assert!(!fx.folder.join("Data/nope.dat").exists());
    let products = fx.state().committed.unwrap().products;
    assert_eq!(products[1].build.as_deref(), Some("100"));
}

fn set_after_store(hook: impl FnMut(&Paths) + 'static) {
    AFTER_STORE.with(|cell| *cell.borrow_mut() = Some(Box::new(hook)));
}

#[test]
fn a_file_whose_md5_comes_with_it_is_named_before_it_reaches_its_path() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    interrupted(&fx, "galaxy.named.after", 0);
    let journal = fx.state().journal.unwrap();
    let md5 = fake::content::md5_hex(b"many chunks and no whole-file md5");
    assert!(journal.attempted.iter().any(|entry| {
        entry.path == "Data/big.dat" && entry.md5.as_deref() == Some(md5.as_str())
    }));
    assert!(!fx.folder.join("Data/big.dat").exists());
    assert!(
        fx.tree()
            .iter()
            .any(|path| path.rsplit('/').next().unwrap().starts_with(".wineo-"))
    );
    fx.install();
    assert_installed(&fx);
}

#[test]
fn finishing_an_install_moves_your_files_aside() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    interrupted(&fx, "galaxy.file.after", 2);
    // Something of yours where the game puts a file, and a folder where it
    // puts another.
    let yours = fx.folder.join("Data/core.dat");
    let _ = fs::remove_file(&yours);
    fs::write(&yours, "yours").unwrap();
    let _ = fs::remove_file(fx.folder.join("empty.txt"));
    fs::create_dir(fx.folder.join("empty.txt")).unwrap();
    fs::write(fx.folder.join("empty.txt/note"), "mine").unwrap();
    let installed = fx.install();
    assert_installed(&fx);
    let saved: Vec<(&str, &str)> = installed
        .saved
        .iter()
        .map(|saved| (saved.path.as_str(), saved.saved_as.as_str()))
        .collect();
    assert_eq!(
        saved,
        [
            ("Data/core.dat", "Data/core.dat.wineo-saved"),
            ("empty.txt", "empty.txt.wineo-saved"),
        ]
    );
    assert_eq!(fx.read("Data/core.dat.wineo-saved"), "yours");
    assert_eq!(fx.read("empty.txt.wineo-saved/note"), "mine");
}
