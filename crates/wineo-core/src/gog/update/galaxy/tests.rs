//! Galaxy installs against the fake content system.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::paths::Paths;

type Hook = Box<dyn FnMut(&Paths)>;
type StepHook = Box<dyn FnMut(&Paths, usize)>;
type PlaceHook = Box<dyn FnMut(&str)>;

thread_local! {
    static AFTER_STORE: RefCell<Option<Hook>> = const { RefCell::new(None) };
    static BEFORE_PLACE: RefCell<Option<PlaceHook>> = const { RefCell::new(None) };
    static FREE_AT: RefCell<Vec<(PathBuf, u64)>> = const { RefCell::new(Vec::new()) };
    static DEVICE_AT: RefCell<Vec<(PathBuf, u64)>> = const { RefCell::new(Vec::new()) };
    static ROUND_LIMITS: RefCell<Option<(u64, usize)>> = const { RefCell::new(None) };
    static BEFORE_SETUP_STEP: RefCell<Option<StepHook>> = const { RefCell::new(None) };
    static SETUP_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Runs a test's hook before each setup step, with how many ran before it
/// on this thread.
pub(super) fn before_setup_step(paths: &Paths) {
    let count = SETUP_STEPS.with(|count| count.replace(count.get() + 1));
    BEFORE_SETUP_STEP.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(paths, count);
        }
    });
}

/// The bytes and chunks a test says a download round ends after.
pub(super) fn round_limits() -> Option<(u64, usize)> {
    ROUND_LIMITS.with(|limits| *limits.borrow())
}

/// Runs a test's hook right before a downloaded game file, or a link, is put
/// in place.
pub(super) fn before_place(path: &str) {
    BEFORE_PLACE.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(path);
        }
    });
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

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
    /// Lists builds the other way round, the last one added first, as GOG
    /// lists a newer build before the one installed.
    newest_first: bool,
    /// Revokes every secure link once this many chunks were served.
    revoke_after: Option<usize>,
    chunks: usize,
    /// Runs on each chunk request with the chunk's compressed MD5, outside
    /// the lock, so it can hold the request while others come in.
    on_chunk: Option<ChunkHook>,
}

type ChunkHook = Arc<dyn Fn(&str) + Send + Sync>;

struct Fixture {
    /// Every setup step run, as `program<TAB>folder<TAB>arguments`.
    cmd_log: PathBuf,
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
        // Setup's batch files run through the shared `cmd` stand-in, which
        // logs each run to `cmd_log`.
        let cmd_log = root.path().join("fake-cmd.log");
        write_script(
            bin.join("wine"),
            format!(
                "#!/bin/sh\nFAKE_CMD_LOG='{}'\n{}case \"$1\" in\nwineboot) [ \"$2\" = --init ] && mkdir -p \"$WINEPREFIX/drive_c\" && echo 'WINE REGISTRY Version 2' > \"$WINEPREFIX/system.reg\" ;;\ncmd) fake_cmd \"$3\"; exit ;;\nesac\nexit 0\n",
                cmd_log.display(),
                crate::test_runners::FAKE_CMD,
            ),
        );
        Self {
            cmd_log,
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
    if ContentSystem::cdn_of(request).is_some() {
        let hook = {
            let mut gog = gog.lock().unwrap();
            gog.chunks += 1;
            if gog.revoke_after == Some(gog.chunks) {
                content.revoke_links(403);
            }
            gog.on_chunk.clone()
        };
        if let Some(hook) = hook {
            hook(path.rsplit('/').next().unwrap_or_default());
        }
        return content
            .answer(request)
            .unwrap_or_else(|| fake::json(404, "{}"));
    }
    let gog = gog.lock().unwrap();
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
        && (!gog.hidden.is_empty() || gog.newest_first)
    {
        let text = String::from_utf8_lossy(bytes);
        let body = &text[text.find("\r\n\r\n").unwrap() + 4..];
        let mut list: serde_json::Value = serde_json::from_str(body).unwrap();
        let items = list["items"].as_array_mut().unwrap();
        items.retain(|item| {
            !gog.hidden
                .contains(item["build_id"].as_str().unwrap_or_default())
        });
        if gog.newest_first {
            items.reverse();
        }
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
    // Every file downloaded: those whose MD5 was known are in place, the
    // rest still wait to be named.
    interrupted(&fx, "galaxy.file.after", 4);
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
    // The store is public: its chunks are asked for without a token.
    let store: Vec<_> = fx
        .server
        .requests()
        .into_iter()
        .filter(|request| request.path().contains("/dependencies/store/"))
        .collect();
    assert!(!store.is_empty());
    for request in store {
        assert!(!request.target.contains("token"), "{}", request.target);
        assert_eq!(request.query("wsSecret"), None);
    }
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
        base: None,
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
    interrupted(&fx, "galaxy.file.after", 4);
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
    // Every file of build 100 is in place, named in the journal.
    interrupted(&fx, "galaxy.files.after", 0);
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
    let info_file = format!("goggame-{GAME}.info");
    let expected: usize = plan
        .manifest_for_tests()
        .files
        .iter()
        .filter(|file| file.path == info_file || file.path == "new.txt")
        .map(|file| file.chunks.len())
        .sum();
    let before = fx.chunk_requests().len();
    let installed = fx.apply(&plan).unwrap();
    // Files that match build 200 stay, the info file the interrupted
    // attempt wrote is replaced as its own, and nothing is moved aside.
    assert_eq!(fx.chunk_requests().len() - before, expected);
    assert!(installed.saved.is_empty());
    assert!(!fx.tree().iter().any(|path| path.contains(".wineo-saved")));
    assert_eq!(fx.read(&info_file), info(GAME, "200"));
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

/// A game planned while an unreferenced redistributable version sits in the
/// store, which cleanup then removes. Returns the plan and the version's
/// folder.
fn planned_over_a_version_cleanup_removes(fx: &Fixture) -> (Plan, PathBuf) {
    fx.content
        .add_build(Fixture::game("100").dependency("MSVC2017"));
    with_dependencies(fx, b"runtime v1");
    // The first plan only learns the version's hash.
    let hash = fx.plan().manifest_for_tests().setup.redist[0]
        .manifest
        .clone();
    let version = fx.paths.gog_redist_dir().join("MSVC2017").join(hash);
    fs::create_dir_all(&version).unwrap();
    let plan = fx.plan();
    assert_eq!(plan.installed_size(), sizes_of_game(&plan));
    assert!(store::clean(&fx.paths).unwrap().len() == 1);
    assert!(!version.exists());
    (plan, version)
}

/// What a plan's game and support files add up to, without the store.
fn sizes_of_game(plan: &Plan) -> u64 {
    let manifest = plan.manifest_for_tests();
    manifest
        .files
        .iter()
        .chain(&manifest.support)
        .map(|file| file.size)
        .sum()
}

#[test]
fn a_version_cleaned_after_planning_still_counts_toward_space() {
    let fx = Fixture::new();
    let (plan, version) = planned_over_a_version_cleanup_removes(&fx);
    // The store is on a filesystem of its own, and has room for nothing.
    let redist = fx.paths.gog_redist_dir();
    DEVICE_AT.with(|list| list.borrow_mut().push((redist.clone(), 9)));
    FREE_AT.with(|list| list.borrow_mut().push((redist, 2)));
    let error = fx.apply(&plan).unwrap_err();
    DEVICE_AT.with(|list| list.borrow_mut().clear());
    FREE_AT.with(|list| list.borrow_mut().clear());
    assert!(error.to_string().starts_with("Not enough space"), "{error}");
    assert!(!fx.folder.exists());
    assert!(folder::peek(&fx.paths, &fx.folder).unwrap().is_none());
    assert!(fx.record().is_none());
    assert!(!fx.paths.environment_config("game").exists());
    assert!(!version.exists());
}

#[test]
fn a_version_cleaned_after_planning_is_downloaded_again_with_room() {
    let fx = Fixture::new();
    let (plan, version) = planned_over_a_version_cleanup_removes(&fx);
    let redist = fx.paths.gog_redist_dir();
    DEVICE_AT.with(|list| list.borrow_mut().push((redist.clone(), 9)));
    FREE_AT.with(|list| list.borrow_mut().push((redist, 1 << 40)));
    let result = fx.apply(&plan);
    DEVICE_AT.with(|list| list.borrow_mut().clear());
    FREE_AT.with(|list| list.borrow_mut().clear());
    result.unwrap();
    assert_installed(&fx);
    assert_eq!(
        fs::read_to_string(version.join("__redist/MSVC2017/VC_redist.x86.exe")).unwrap(),
        "runtime v1"
    );
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

fn set_before_place(hook: impl FnMut(&str) + 'static) {
    BEFORE_PLACE.with(|cell| *cell.borrow_mut() = Some(Box::new(hook)));
}

fn clear_before_place() {
    BEFORE_PLACE.with(|cell| *cell.borrow_mut() = None);
}

#[test]
fn a_file_put_where_one_is_downloading_is_saved_not_overwritten() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    let target = fx.folder.join("bin/Game.exe");
    set_before_place(move |path| {
        if path == "bin/Game.exe" {
            fs::write(&target, "put here meanwhile").unwrap();
        }
    });
    let installed = fx.install();
    clear_before_place();
    assert_installed(&fx);
    assert_eq!(
        installed
            .saved
            .iter()
            .map(|saved| (saved.path.as_str(), saved.saved_as.as_str()))
            .collect::<Vec<_>>(),
        [("bin/Game.exe", "bin/Game.exe.wineo-saved")]
    );
    assert_eq!(fx.read("bin/Game.exe.wineo-saved"), "put here meanwhile");
}

/// Build 100 in place but unfinished, its record lost, and GOG listing
/// only build 200, whose info file differs: the one build 100's attempt
/// wrote is wineo's own to replace.
fn retargeted(fx: &Fixture) -> Plan {
    fx.content.add_build(Fixture::game("100"));
    interrupted(fx, "galaxy.files.after", 0);
    fs::remove_file(folder::sidecar_file(&fx.paths, &fx.folder)).unwrap();
    fx.hide("100");
    fx.content.add_build(Fixture::game("200"));
    fx.plan()
}

#[test]
fn a_file_of_wineos_changed_while_downloading_is_saved() {
    let fx = Fixture::new();
    let plan = retargeted(&fx);
    let info_file = format!("goggame-{GAME}.info");
    let (path, watched) = (fx.folder.join(&info_file), info_file.clone());
    set_before_place(move |placing| {
        if placing == watched {
            fs::write(&path, "changed by you").unwrap();
        }
    });
    let installed = fx.apply(&plan).unwrap();
    clear_before_place();
    assert_eq!(installed.saved.len(), 1);
    assert_eq!(installed.saved[0].path, info_file);
    assert_eq!(
        fx.read(&format!("{info_file}.wineo-saved")),
        "changed by you"
    );
    assert_eq!(fx.read(&info_file), info(GAME, "200"));
}

#[test]
fn a_crash_with_wineos_file_moved_aside_deletes_it_only_on_a_hash_match() {
    for changed in [false, true] {
        let fx = Fixture::new();
        let plan = retargeted(&fx);
        set_failpoint(Some("galaxy.aside.after"));
        let error = fx.apply(&plan).unwrap_err();
        set_failpoint(None);
        assert!(format!("{error:#}").contains("injected"), "{error:#}");
        let info_file = format!("goggame-{GAME}.info");
        let aside = fx
            .tree()
            .into_iter()
            .find(|path| path.contains(".wineo-aside-"))
            .unwrap();
        assert!(aside.starts_with(&info_file), "{aside}");
        if changed {
            fs::write(fx.folder.join(&aside), "changed by you").unwrap();
        }
        let installed = fx.install();
        assert!(!fx.tree().iter().any(|path| path.contains(".wineo-aside-")));
        assert_eq!(fx.read(&info_file), info(GAME, "200"));
        let saved = fx.folder.join(format!("{info_file}.wineo-saved"));
        assert_eq!(saved.exists(), changed);
        assert_eq!(installed.saved.len(), usize::from(changed));
        if changed {
            assert_eq!(fs::read_to_string(saved).unwrap(), "changed by you");
        }
    }
}

#[test]
fn a_file_put_where_a_link_goes_is_saved_not_overwritten() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    let target = fx.folder.join("Data/current");
    set_before_place(move |path| {
        if path == "Data/current" {
            fs::write(&target, "put here meanwhile").unwrap();
        }
    });
    let installed = fx.install();
    clear_before_place();
    assert_installed(&fx);
    assert_eq!(
        installed
            .saved
            .iter()
            .map(|saved| (saved.path.as_str(), saved.saved_as.as_str()))
            .collect::<Vec<_>>(),
        [("Data/current", "Data/current.wineo-saved")]
    );
    assert_eq!(fx.read("Data/current.wineo-saved"), "put here meanwhile");
}

/// Build 100 in place but unfinished, its record lost, and GOG listing only
/// build 200, which ships `changed` on top of the game: `Data/swap` is a
/// file of wineo's when build 200 wants something else there.
fn retargeted_to(fx: &Fixture, changed: FakeDepot) -> Plan {
    fx.content.add_build(
        Fixture::game("100").depot(
            FakeDepot::new(GAME).file(FakeFile::new("Data\\swap", b"wineo's swap").with_md5()),
        ),
    );
    interrupted(fx, "galaxy.files.after", 0);
    fs::remove_file(folder::sidecar_file(&fx.paths, &fx.folder)).unwrap();
    fx.hide("100");
    fx.content.add_build(Fixture::game("200").depot(changed));
    fx.plan()
}

/// Applies `plan`, crashing first right after the entry at `path` is renamed
/// out of the way when `crash` says so, which shows it is renamed before it
/// is checked.
fn apply_through_aside(fx: &Fixture, plan: &Plan, path: &str, crash: bool) -> Installed {
    if !crash {
        return fx.apply(plan).unwrap();
    }
    set_failpoint(Some("galaxy.aside.after"));
    let error = fx.apply(plan).unwrap_err();
    set_failpoint(None);
    assert!(format!("{error:#}").contains("injected"), "{error:#}");
    let asides: Vec<String> = fx
        .tree()
        .into_iter()
        .filter(|entry| entry.contains(".wineo-aside-"))
        .collect();
    assert_eq!(asides.len(), 1, "{asides:?}");
    assert!(
        asides[0].starts_with(&format!("{path}.wineo-aside-")),
        "{asides:?}"
    );
    assert!(fs::symlink_metadata(fx.folder.join(path)).is_err());
    fx.install()
}

#[test]
fn a_file_of_wineos_where_a_link_goes_is_deleted_only_on_a_hash_match() {
    for changed in [false, true] {
        for crash in [false, true] {
            let fx = Fixture::new();
            let plan = retargeted_to(&fx, FakeDepot::new(GAME).link("Data\\swap", "core.dat"));
            if changed {
                fs::write(fx.folder.join("Data/swap"), "changed by you").unwrap();
            }
            let installed = apply_through_aside(&fx, &plan, "Data/swap", crash);
            assert!(!fx.tree().iter().any(|path| path.contains(".wineo-aside-")));
            assert_eq!(
                fs::read_link(fx.folder.join("Data/swap")).unwrap(),
                Path::new("core.dat")
            );
            let saved = fx.folder.join("Data/swap.wineo-saved");
            assert_eq!(saved.exists(), changed, "changed {changed}, crash {crash}");
            assert_eq!(installed.saved.len(), usize::from(changed));
            if changed {
                assert_eq!(fs::read_to_string(saved).unwrap(), "changed by you");
            }
        }
    }
}

#[test]
fn a_link_of_wineos_is_deleted_only_while_it_points_where_recorded() {
    for changed in [false, true] {
        for crash in [false, true] {
            let fx = Fixture::new();
            let plan = retargeted_to(
                &fx,
                FakeDepot::new(GAME).file(FakeFile::new("Data\\current", b"a file now")),
            );
            if changed {
                fs::remove_file(fx.folder.join("Data/current")).unwrap();
                std::os::unix::fs::symlink("big.dat", fx.folder.join("Data/current")).unwrap();
            }
            let installed = apply_through_aside(&fx, &plan, "Data/current", crash);
            assert!(!fx.tree().iter().any(|path| path.contains(".wineo-aside-")));
            assert_eq!(fx.read("Data/current"), "a file now");
            let saved = fx.folder.join("Data/current.wineo-saved");
            assert_eq!(
                fs::read_link(&saved).ok(),
                changed.then(|| PathBuf::from("big.dat")),
                "changed {changed}, crash {crash}"
            );
            assert_eq!(installed.saved.len(), usize::from(changed));
        }
    }
}

#[test]
fn galaxy_installs_refuse_a_filesystem_that_cant_rename_without_replacing() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    interrupted(&fx, "galaxy.files.after", 0);
    fs::write(fx.folder.join("Data/core.dat"), "yours").unwrap();
    let plan = fx.plan();
    crate::gog::update::publish::set_no_rename_noreplace(true);
    let error = fx.apply(&plan).unwrap_err();
    crate::gog::update::publish::set_no_rename_noreplace(false);
    assert!(
        format!("{error:#}").contains("can't rename a file without replacing"),
        "{error:#}"
    );
    assert_eq!(fx.read("Data/core.dat"), "yours");
    assert!(!fx.tree().iter().any(|path| path.contains(".wineo-saved")));
    let installed = fx.install();
    assert_installed(&fx);
    assert_eq!(installed.saved.len(), 1);
    assert_eq!(fx.read("Data/core.dat.wineo-saved"), "yours");
}

#[test]
fn same_size_damage_counts_toward_space() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    interrupted(&fx, "galaxy.files.after", 0);
    let damaged = "core data, several chunks LONG";
    assert_eq!(damaged.len(), "core data, several chunks long".len());
    fs::write(fx.folder.join("Data/core.dat"), damaged).unwrap();
    let state = fx.state();
    let record = fs::read(folder::sidecar_file(&fx.paths, &fx.folder)).unwrap();
    let plan = fx.plan();
    FREE_AT.with(|list| list.borrow_mut().push((fx.folder.clone(), 10)));
    let error = fx.apply(&plan).unwrap_err();
    FREE_AT.with(|list| list.borrow_mut().clear());
    assert!(error.to_string().starts_with("Not enough space"), "{error}");
    assert!(error.to_string().contains("needs 30 B"), "{error}");
    assert_eq!(fx.state(), state);
    assert_eq!(
        fs::read(folder::sidecar_file(&fx.paths, &fx.folder)).unwrap(),
        record
    );
    assert_eq!(fx.read("Data/core.dat"), damaged);
}

#[test]
fn finishing_syncs_support_folders_an_earlier_attempt_filled() {
    let fx = Fixture::new();
    fx.content.add_build(
        Fixture::game("100").depot(
            FakeDepot::new(GAME)
                .file(FakeFile::new("__support\\setup.exe", b"setup program").flag("support")),
        ),
    );
    interrupted(&fx, "galaxy.support.placed", 0);
    assert!(fx.support(GAME, "__support/setup.exe").is_file());
    let _ = crate::gog::update::publish::take_syncs();
    fx.install();
    assert_installed(&fx);
    let syncs = crate::gog::update::publish::take_syncs();
    let commit = syncs.iter().position(|event| event == "commit").unwrap();
    for dir in [
        format!("support:{GAME}/__support"),
        format!("support:{GAME}"),
        "support:".to_owned(),
    ] {
        let at = syncs.iter().position(|event| *event == dir);
        assert!(at.is_some_and(|at| at < commit), "{dir}: {syncs:?}");
    }
}

#[test]
fn a_record_breaking_recovery_rules_reads_as_none() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    interrupted(&fx, "galaxy.files.after", 0);
    let good: serde_json::Value =
        serde_json::from_slice(&fs::read(folder::sidecar_file(&fx.paths, &fx.folder)).unwrap())
            .unwrap();
    assert!(Record::parse(good.to_string().as_bytes()).is_some());
    let mut escaping = good.clone();
    escaping["pending"]["manifest"]["links"][0]["target"] = value!("../../../etc");
    assert!(Record::parse(escaping.to_string().as_bytes()).is_none());
    let mut absolute = good.clone();
    absolute["pending"]["manifest"]["links"][0]["target"] = value!("/etc");
    assert!(Record::parse(absolute.to_string().as_bytes()).is_none());
    let mut support = good.clone();
    support["pending"]["manifest"]["support"] = value!([{
        "path": "setup.exe",
        "dependency": "DOSBOX",
        "size": 0,
        "chunks": [],
    }]);
    assert!(Record::parse(support.to_string().as_bytes()).is_none());
    let mut drive = good.clone();
    drive["pending"]["manifest"]["files"][0]["path"] = value!("C:/x");
    assert!(Record::parse(drive.to_string().as_bytes()).is_none());

    // Recovery with such a record falls back to fetching the build again.
    fs::write(
        folder::sidecar_file(&fx.paths, &fx.folder),
        escaping.to_string(),
    )
    .unwrap();
    fx.install();
    assert_installed(&fx);
}

#[test]
fn an_unstarted_operations_pending_never_makes_a_file_tracked() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    // The install's journal is written, nothing placed yet.
    interrupted(&fx, "galaxy.journal.after", 0);
    let state = fx.state();
    // A file of yours, and a pending of an operation that never started
    // claiming it as tracked at exactly that content.
    fs::create_dir_all(fx.folder.join("Data")).unwrap();
    fs::write(fx.folder.join("Data/core.dat"), "yours").unwrap();
    let mut record = fx.record().unwrap();
    let pending = record.pending.as_mut().unwrap();
    pending.operation = "20261014T000000-9".to_owned();
    for file in &mut pending.manifest.files {
        if file.path == "Data/core.dat" {
            file.md5 = Some(fake::content::md5_hex(b"yours"));
        }
    }
    write_record(&fx, &record);
    assert!(record::pending(&state, &fx.record().unwrap()).is_none());

    let installed = fx.install();
    assert_installed(&fx);
    assert_eq!(fx.read("Data/core.dat.wineo-saved"), "yours");
    assert!(
        installed
            .saved
            .iter()
            .any(|saved| saved.path == "Data/core.dat")
    );
}

fn set_round_limits(limits: Option<(u64, usize)>) {
    ROUND_LIMITS.with(|cell| *cell.borrow_mut() = limits);
}

impl Fixture {
    fn on_chunk(&self, hook: impl Fn(&str) + Send + Sync + 'static) {
        self.gog.lock().unwrap().on_chunk = Some(Arc::new(hook));
    }

    /// The game with `huge` added: a file of `count` chunks, first in the
    /// download order, whose chunks no other file shares.
    fn with_huge(&self, count: usize, file: impl FnOnce(FakeFile) -> FakeFile) -> Vec<u8> {
        let bytes = huge(count);
        self.content.add_build(
            Fixture::game("100")
                .depot(FakeDepot::new(GAME).file(file(FakeFile::new(HUGE, &bytes)))),
        );
        bytes
    }
}

const HUGE: &str = "Big\\huge.dat";

fn huge(count: usize) -> Vec<u8> {
    (0..count)
        .flat_map(|index| format!("huge{index:04}").into_bytes())
        .collect()
}

/// The compressed MD5 of the huge file's chunk `index`, as requested.
fn huge_chunk(index: usize) -> String {
    fake::content::md5_hex(&fake::content::zlib(format!("huge{index:04}").as_bytes()))
}

/// What a test has seen so far, for requests held until something else
/// happens.
#[derive(Default)]
struct Seen {
    list: Mutex<Vec<String>>,
    changed: Condvar,
}

impl Seen {
    fn add(&self, what: &str) {
        self.list.lock().unwrap().push(what.to_owned());
        self.changed.notify_all();
    }

    /// Waits until `ready` holds. False when it doesn't in time, which only
    /// happens when downloads aren't as concurrent as they should be.
    fn wait_for(&self, ready: impl Fn(&[String]) -> bool) -> bool {
        let list = self.list.lock().unwrap();
        let (list, waited) = self
            .changed
            .wait_timeout_while(list, Duration::from_secs(10), |list| !ready(list))
            .unwrap();
        drop(list);
        !waited.timed_out()
    }
}

#[test]
fn one_files_chunks_download_at_once() {
    let fx = Fixture::new();
    let bytes = fx.with_huge(12, |file| file);
    let hashes: Vec<String> = (0..12).map(huge_chunk).collect();
    let seen = Arc::new(Seen::default());
    let missed = Arc::new(AtomicBool::new(false));
    {
        let (seen, missed) = (Arc::clone(&seen), Arc::clone(&missed));
        fx.on_chunk(move |hash| {
            if !hashes.iter().any(|known| known == hash) {
                return;
            }
            seen.add(hash);
            // Held until every worker is on one of this file's chunks.
            if !seen.wait_for(|list| list.len() >= fetch::WORKERS) {
                missed.store(true, Ordering::Relaxed);
            }
        });
    }
    fx.install();
    assert_installed(&fx);
    assert_eq!(fs::read(fx.folder.join("Big/huge.dat")).unwrap(), bytes);
    assert!(!missed.load(Ordering::Relaxed));
}

#[test]
fn small_files_finish_while_a_big_one_downloads() {
    let fx = Fixture::new();
    let bytes = huge(12);
    // More small files than the old rounds of 64 files held.
    let mut depot = FakeDepot::new(GAME).file(FakeFile::new(HUGE, &bytes));
    for index in 0..70 {
        depot = depot.file(FakeFile::new(
            &format!("small\\{index:02}.txt"),
            format!("s{index:02}").as_bytes(),
        ));
    }
    fx.content.add_build(Fixture::game("100").depot(depot));
    let placed = Arc::new(Seen::default());
    let missed = Arc::new(AtomicBool::new(false));
    {
        let placed = Arc::clone(&placed);
        set_before_place(move |path| placed.add(path));
    }
    {
        let (placed, missed) = (Arc::clone(&placed), Arc::clone(&missed));
        let first = huge_chunk(0);
        // The big file's first chunk waits for every small file to be in
        // place.
        fx.on_chunk(move |hash| {
            if hash == first
                && !placed.wait_for(|list| {
                    list.iter()
                        .filter(|path| path.starts_with("small/"))
                        .count()
                        == 70
                })
            {
                missed.store(true, Ordering::Relaxed);
            }
        });
    }
    let result = fx.apply(&fx.plan());
    clear_before_place();
    result.unwrap();
    assert_installed(&fx);
    assert_eq!(fs::read(fx.folder.join("Big/huge.dat")).unwrap(), bytes);
    assert_eq!(fx.read("small/69.txt"), "s69");
    assert!(!missed.load(Ordering::Relaxed));
}

#[test]
fn chunks_written_out_of_order_make_the_whole_file() {
    let fx = Fixture::new();
    let bytes = fx.with_huge(12, |file| file.with_md5().with_sha256());
    let seen = Arc::new(Seen::default());
    let missed = Arc::new(AtomicBool::new(false));
    {
        let (seen, missed) = (Arc::clone(&seen), Arc::clone(&missed));
        let (first, later) = (huge_chunk(0), huge_chunk(fetch::WORKERS));
        // A worker asks for a chunk past the first few only once it wrote
        // one of them, so the first chunk is written after a later one.
        fx.on_chunk(move |hash| {
            seen.add(hash);
            if hash == first && !seen.wait_for(|list| list.contains(&later)) {
                missed.store(true, Ordering::Relaxed);
            }
        });
    }
    fx.install();
    assert_installed(&fx);
    assert_eq!(fs::read(fx.folder.join("Big/huge.dat")).unwrap(), bytes);
    let (state, record) = (fx.state(), fx.record().unwrap());
    let installed = record::installed(&state, &record).unwrap().unwrap();
    let entry = installed
        .manifest
        .files
        .iter()
        .find(|file| file.path == "Big/huge.dat")
        .unwrap();
    assert_eq!(
        entry.md5.as_deref(),
        Some(fake::content::md5_hex(&bytes).as_str())
    );
    assert!(!missed.load(Ordering::Relaxed));
}

#[test]
fn a_file_spanning_rounds_comes_out_whole() {
    let fx = Fixture::new();
    let bytes = fx.with_huge(12, |file| file);
    // Rounds of three chunks, so the huge file spans four.
    set_round_limits(Some((24, 3)));
    let result = fx.apply(&fx.plan());
    set_round_limits(None);
    result.unwrap();
    assert_installed(&fx);
    assert_eq!(fs::read(fx.folder.join("Big/huge.dat")).unwrap(), bytes);
}

#[test]
fn a_round_never_goes_over_its_byte_cap() {
    let mib = 1 << 20;
    let rounds = |sizes: &[u64], max_bytes: u64, max_units: usize| {
        let mut ends = Vec::new();
        let mut start = 0;
        while start < sizes.len() {
            start = super::fetch::round_end(
                sizes.len(),
                start,
                |unit| sizes[unit],
                max_bytes,
                max_units,
            );
            ends.push(start);
        }
        ends
    };
    // Two 600 MiB chunks would make 1.2 GiB, so each gets its own round.
    assert_eq!(
        rounds(&[600 * mib, 600 * mib, 100 * mib], 1024 * mib, 1024),
        [1, 3]
    );
    // Exactly the cap still fits.
    assert_eq!(rounds(&[512 * mib, 512 * mib, 1], 1024 * mib, 1024), [2, 3]);
    // A chunk over the cap still gets a round of its own.
    assert_eq!(rounds(&[2048 * mib, 1], 1024 * mib, 1024), [1, 2]);
    // The unit cap holds when bytes don't.
    assert_eq!(rounds(&[1; 5], 1024 * mib, 2), [2, 4, 5]);
}

#[test]
fn a_corrupt_chunk_mid_file_fails_the_install() {
    let fx = Fixture::new();
    fx.with_huge(12, |file| file);
    fx.content.corrupt_chunk(&huge_chunk(7));
    let error = fx.apply(&fx.plan()).unwrap_err();
    let shown = format!("{error:#}");
    assert!(shown.contains("Big/huge.dat"), "{shown}");
    assert!(!fx.folder.join("Big/huge.dat").exists());
    assert!(fx.state().journal.is_some());
}

#[test]
fn an_install_canceled_mid_file_is_finished_by_installing_again() {
    let fx = Fixture::new();
    let bytes = fx.with_huge(12, |file| file);
    let token = Token::new();
    {
        let (token, middle) = (token.clone(), huge_chunk(5));
        fx.on_chunk(move |hash| {
            if hash == middle {
                token.cancel();
            }
        });
    }
    let error =
        install::apply(&fx.paths, &mut fx.session(), &fx.plan(), &token, |_| {}).unwrap_err();
    assert!(
        matches!(
            error.downcast_ref(),
            Some(crate::gog::update::UpdateError::Canceled { journal: true })
        ),
        "{error:#}"
    );
    assert!(!fx.folder.join("Big/huge.dat").exists());
    assert!(fx.state().journal.is_some());
    // The half-written file is still there under its temporary name.
    assert!(
        fx.tree().iter().any(|path| path.starts_with("Big/.wineo-")),
        "{:?}",
        fx.tree()
    );

    fx.gog.lock().unwrap().on_chunk = None;
    let plan = fx.plan();
    assert!(plan.resumes());
    let left = chunks_left(&fx, &plan);
    let before = fx.chunk_requests().len();
    fx.apply(&plan).unwrap();
    assert_installed(&fx);
    assert_eq!(fs::read(fx.folder.join("Big/huge.dat")).unwrap(), bytes);
    // The half-written file counts for nothing: all of it comes again.
    assert_eq!(fx.chunk_requests().len() - before, left);
}

mod repair;
mod setup;
mod update;
