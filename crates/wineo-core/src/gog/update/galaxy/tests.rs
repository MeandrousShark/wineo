//! Galaxy installs against the fake content system.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::paths::Paths;

type Hook = Box<dyn FnMut(&Paths)>;

thread_local! {
    static AFTER_STORE: RefCell<Option<Hook>> = const { RefCell::new(None) };
    static STORE_STEP: RefCell<Option<Box<dyn FnMut(&'static str)>>> = const { RefCell::new(None) };
    static FREE_AT: RefCell<Vec<(PathBuf, u64)>> = const { RefCell::new(Vec::new()) };
    static DEVICE_AT: RefCell<Vec<(PathBuf, u64)>> = const { RefCell::new(Vec::new()) };
}

/// Runs a test's hook once the store versions are held and downloaded.
pub(super) fn after_store(paths: &Paths) {
    if let Some(mut hook) = AFTER_STORE.with(|hook| hook.borrow_mut().take()) {
        hook(paths);
    }
}

/// Runs a test's hook at a step of store cleanup.
pub(super) fn store_step(step: &'static str) {
    STORE_STEP.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(step);
        }
    });
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
}

struct Fixture {
    root: TempDir,
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
            root,
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
        !fx.tree().iter().any(|path| path.contains(".wineo-")),
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
