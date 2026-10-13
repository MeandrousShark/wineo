use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use md5::{Digest as _, Md5};
use sha2::Sha256;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::*;
use crate::gog::account;
use crate::gog::folder::{InUseLock, failpoint_armed, set_failpoint, set_failpoint_nth};
use crate::gog::test_support::{FakeKeyring, Scripted, client_at, setup, tokens};
use crate::gog::update::publish::take_syncs;
use crate::steam::api::fixture::reply;

const RIMWORLD: u64 = 1094900565;
const ROYALTY: u64 = 1233017772;
const IDEOLOGY: u64 = 1827857764;
const USER: u64 = 1001;
const SCRIPT: &[u8] = b"#!/bin/bash\n# a stand-in for GOG's split file script\n";
const MIB: usize = 1024 * 1024;

/// A package's size as GOG's details list it: rounded to whole MiB, down
/// like a big file's usually is, and up for the small ones here, which
/// GOG would otherwise list as empty.
fn listed(size: usize) -> usize {
    if size >= MIB {
        size / MIB * MIB
    } else {
        size.div_ceil(MIB) * MIB
    }
}

/// Called at each step of `remove_packages`.
type CleanupHook = Box<dyn FnMut(&'static str)>;

thread_local! {
    static FREE: Cell<Option<u64>> = const { Cell::new(None) };
    static BEFORE_JOURNAL: RefCell<Option<Box<dyn FnMut()>>> = const { RefCell::new(None) };
    static CLEANUP: RefCell<Option<CleanupHook>> = const { RefCell::new(None) };
}

/// Runs a test's hook at a step of removing packages after an update.
pub(in crate::gog::update) fn cleanup_step(step: &'static str) {
    CLEANUP.with(|hook| {
        if let Some(hook) = hook.borrow_mut().as_mut() {
            hook(step);
        }
    });
}

/// Runs `remove_packages` on another thread, which reports each step on
/// `steps` and waits for a word on the returned sender before carrying on.
fn cleanup_in_steps<'scope>(
    scope: &'scope std::thread::Scope<'scope, '_>,
    paths: Paths,
    product: u64,
    steps: std::sync::mpsc::Sender<&'static str>,
) -> (
    std::sync::mpsc::Sender<()>,
    std::thread::ScopedJoinHandle<'scope, Vec<String>>,
) {
    let (go, wait) = std::sync::mpsc::channel::<()>();
    let handle = scope.spawn(move || {
        CLEANUP.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |step| {
                steps.send(step).unwrap();
                wait.recv().unwrap();
            }))
        });
        let mut warnings = Vec::new();
        remove_packages(&paths, Path::new("/elsewhere"), &[product], &mut warnings);
        warnings
    });
    (go, handle)
}

/// Runs a test's hook once, right before an update writes its journal.
pub(in crate::gog::update) fn before_journal() {
    if let Some(mut hook) = BEFORE_JOURNAL.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
}

pub(in crate::gog::update) fn known_scripts() -> Vec<[u8; 32]> {
    vec![Sha256::digest(SCRIPT).into()]
}

pub(in crate::gog::update) fn free_space() -> Option<u64> {
    FREE.with(Cell::get)
}

/// One entry in a fixture package, under `data/noarch/`.
#[derive(Clone)]
enum Item {
    File(&'static str, Vec<u8>),
    Exe(&'static str, Vec<u8>),
    Link(&'static str, &'static str),
    /// A file GOG ships as pieces, joined by the setup script.
    Split(&'static str, Vec<u8>),
}

fn file(path: &'static str, content: &str) -> Item {
    Item::File(path, content.as_bytes().to_vec())
}

/// A `.sh` built like GOG's: a shell header, a MojoSetup stand-in, a zip.
fn package(items: &[Item]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = |mode: u32| {
        SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(mode)
    };
    let mut splits = Vec::new();
    for item in items {
        match item {
            Item::File(path, content) | Item::Exe(path, content) => {
                let mode = if matches!(item, Item::Exe(..)) {
                    0o755
                } else {
                    0o644
                };
                zip.start_file(format!("data/noarch/{path}"), options(mode))
                    .unwrap();
                zip.write_all(content).unwrap();
            }
            Item::Link(path, target) => zip
                .add_symlink(format!("data/noarch/{path}"), *target, options(0o777))
                .unwrap(),
            Item::Split(path, content) => {
                let middle = content.len() / 2;
                for (number, piece) in [&content[..middle], &content[middle..]].iter().enumerate() {
                    zip.start_file(
                        format!("data/noarch/{path}.split{number:02}"),
                        options(0o644),
                    )
                    .unwrap();
                    zip.write_all(piece).unwrap();
                }
                splits.push(*path);
            }
        }
    }
    if !splits.is_empty() {
        zip.start_file("data/noarch/support/postinst.sh", options(0o755))
            .unwrap();
        zip.write_all(SCRIPT).unwrap();
        zip.start_file("data/noarch/support/split_files", options(0o644))
            .unwrap();
        zip.write_all(splits.join("\n").as_bytes()).unwrap();
    }
    let zipped = zip.finish().unwrap().into_inner();
    let mut bytes = b"#!/bin/sh\n# Makeself stand-in\nexit 0\n".to_vec();
    bytes.extend(b"MOJOSETUP".repeat(50));
    bytes.extend(zipped);
    bytes
}

fn md5_hex(bytes: &[u8]) -> String {
    Md5::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// What GOG offers for one product.
#[derive(Clone)]
struct Offer {
    title: &'static str,
    version: String,
    /// None: no Linux installer.
    bytes: Option<Vec<u8>>,
    checksum: bool,
    /// GOG's code for the installer's language.
    language: &'static str,
}

#[derive(Default)]
struct Gog {
    offers: BTreeMap<u64, Offer>,
    /// Products listed as DLC of the base, in order.
    dlc: Vec<u64>,
    downloads: usize,
    /// The products the account owns. None owns every product offered.
    owned: Option<Vec<u64>>,
    /// Whether the owned ids list fails.
    owned_ids_fail: bool,
    /// Products whose package GOG fails to serve.
    failing: Vec<u64>,
}

impl Gog {
    fn name(id: u64, version: &str) -> String {
        format!("game_{id}_{}.sh", version.replace(['.', ' '], "_"))
    }

    fn json(&self, id: u64, base: &str) -> serde_json::Value {
        let offer = &self.offers[&id];
        let installers: Vec<serde_json::Value> = offer
            .bytes
            .iter()
            .map(|bytes| {
                let name = Self::name(id, &offer.version);
                serde_json::json!({
                    "id": format!("installer_linux_{}", offer.language),
                    "os": "linux",
                    "language": offer.language,
                    "version": offer.version,
                    "total_size": listed(bytes.len()),
                    "files": [{
                        "id": "en3installer0",
                        "size": listed(bytes.len()),
                        "downlink": format!("{base}/downlink/{name}"),
                    }],
                })
            })
            .collect();
        serde_json::json!({
            "id": id,
            "title": offer.title,
            "downloads": {"installers": installers},
        })
    }

    fn answer(&mut self, line: &str, base: &str) -> Vec<u8> {
        let path = line.split(' ').nth(1).unwrap_or_default();
        let path = path.split('?').next().unwrap_or_default();
        let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
        match parts.as_slice() {
            ["token"] => tokens("ACCESS", "R", USER, 3600),
            ["products", id] => {
                let Ok(id) = id.parse::<u64>() else {
                    return reply(404, "{}");
                };
                if !self.offers.contains_key(&id) {
                    return reply(404, "{}");
                }
                let mut json = self.json(id, base);
                if id == RIMWORLD {
                    json["dlcs"] = serde_json::json!({
                        "products": self
                            .dlc
                            .iter()
                            .map(|dlc| serde_json::json!({"id": dlc}))
                            .collect::<Vec<_>>(),
                    });
                    json["expanded_dlcs"] = self
                        .dlc
                        .iter()
                        .filter(|dlc| self.offers.contains_key(dlc))
                        .map(|dlc| self.json(*dlc, base))
                        .collect();
                }
                reply(200, &json.to_string())
            }
            ["downlink", name] => {
                let sum = self
                    .find(name)
                    .is_some_and(|offer| offer.checksum)
                    .then(|| format!("{base}/sums/{name}"));
                let body = serde_json::json!({
                    "downlink": format!("{base}/files/{name}"),
                    "checksum": sum,
                });
                reply(200, &body.to_string())
            }
            ["sums", name] => match self.find(name).and_then(|offer| offer.bytes.clone()) {
                Some(bytes) => {
                    let body = format!(
                        r#"<file name="{name}" md5="{}" total_size="{}"/>"#,
                        md5_hex(&bytes),
                        bytes.len()
                    );
                    reply(200, &body)
                }
                None => reply(404, "{}"),
            },
            ["user", "data", "games"] if self.owned_ids_fail => reply(500, "{}"),
            ["user", "data", "games"] => {
                let owned = self
                    .owned
                    .clone()
                    .unwrap_or_else(|| self.offers.keys().copied().collect());
                reply(200, &serde_json::json!({ "owned": owned }).to_string())
            }
            ["files", name] if self.failing.iter().any(|id| self.is_file_of(*id, name)) => {
                reply(500, "{}")
            }
            ["files", name] => match self.find(name).and_then(|offer| offer.bytes.clone()) {
                Some(bytes) => {
                    self.downloads += 1;
                    let mut answer = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    )
                    .into_bytes();
                    answer.extend(bytes);
                    answer
                }
                None => reply(404, "{}"),
            },
            _ => reply(404, "{}"),
        }
    }

    fn is_file_of(&self, id: u64, name: &str) -> bool {
        self.offers
            .get(&id)
            .is_some_and(|offer| Self::name(id, &offer.version) == name)
    }

    fn find(&self, name: &str) -> Option<&Offer> {
        self.offers
            .iter()
            .find(|(id, offer)| Self::name(**id, &offer.version) == name)
            .map(|(_, offer)| offer)
    }
}

struct Fixture {
    root: TempDir,
    paths: Paths,
    keyring: Arc<FakeKeyring>,
    gog: Arc<Mutex<Gog>>,
    server: Scripted,
    folder: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let (root, paths) = setup();
        let keyring = Arc::new(FakeKeyring::default());
        let secret = format!(r#"{{"refresh_token":"R","user_id":"{USER}"}}"#);
        crate::key_store::save_key(&account::KEY_SPEC, &paths, &*keyring, &secret).unwrap();
        let gog = Arc::new(Mutex::new(Gog::default()));
        let base = Arc::new(OnceLock::<String>::new());
        let server = {
            let (gog, base) = (Arc::clone(&gog), Arc::clone(&base));
            Scripted::start(move |line| gog.lock().unwrap().answer(line, base.get().unwrap()))
        };
        base.set(server.base.clone()).unwrap();
        let folder = fs::canonicalize(root.path())
            .unwrap()
            .join("Games")
            .join("RimWorld");
        fs::create_dir_all(&folder).unwrap();
        let fx = Self {
            root,
            paths,
            keyring,
            gog,
            server,
            folder,
        };
        fx.config("rimworld", &fx.folder.join("start.sh"));
        fx
    }

    fn config(&self, name: &str, exe: &Path) {
        let path = self.paths.environment_config(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!(
                "kind = \"native\"\ngog_id = \"{RIMWORLD}\"\n\n[targets.game]\nexe = \"{}\"\n",
                exe.display()
            ),
        )
        .unwrap();
    }

    fn offer(&self, id: u64, title: &'static str, version: &str, items: &[Item]) {
        let mut gog = self.gog.lock().unwrap();
        gog.offers.insert(
            id,
            Offer {
                title,
                version: version.to_owned(),
                bytes: Some(package(items)),
                checksum: true,
                language: "en",
            },
        );
        if id != RIMWORLD && !gog.dlc.contains(&id) {
            gog.dlc.push(id);
        }
    }

    /// Makes a product's package over a MiB, so GOG lists it smaller than
    /// it is. The padding goes before the zip, like the rest of the header.
    fn pad(&self, id: u64) {
        let mut gog = self.gog.lock().unwrap();
        let bytes = gog.offers.get_mut(&id).unwrap().bytes.as_mut().unwrap();
        let at = bytes
            .windows(9)
            .position(|window| window == b"MOJOSETUP")
            .unwrap();
        bytes.splice(at..at, b"MOJOSETUP".repeat(MIB / 9 * 5 / 4));
        assert!(listed(bytes.len()) < bytes.len());
    }

    fn package_size(&self, id: u64) -> u64 {
        self.gog.lock().unwrap().offers[&id]
            .bytes
            .as_ref()
            .unwrap()
            .len() as u64
    }

    fn withdraw(&self, id: u64) {
        self.gog.lock().unwrap().offers.get_mut(&id).unwrap().bytes = None;
    }

    fn downloads(&self) -> usize {
        self.gog.lock().unwrap().downloads
    }

    /// A state like a migration leaves: products with versions, no
    /// winning files.
    fn migrated(&self, products: &[(u64, Option<&str>)]) {
        let state = FolderState {
            committed: Some(Committed {
                gog_id: RIMWORLD,
                platform: Platform::Linux,
                language: "en-US".to_owned(),
                track: Track::Installer,
                products: products
                    .iter()
                    .map(|(id, version)| Product {
                        id: *id,
                        version: version.map(str::to_owned),
                        build: None,
                        language: None,
                        status: ProductStatus::Installed,
                    })
                    .collect(),
                files: Vec::new(),
            }),
            ..FolderState::new(self.folder.clone(), Some("rimworld".to_owned()))
        };
        self.save(&state);
    }

    fn save(&self, state: &FolderState) {
        FolderLock::exclusive(&self.paths, &state.folder, &|| false)
            .unwrap()
            .write(state)
            .unwrap();
    }

    fn state(&self) -> FolderState {
        folder::peek(&self.paths, &self.folder).unwrap().unwrap()
    }

    fn session(&self) -> Session<'_> {
        Session::open(&self.paths, &*self.keyring, client_at(&self.server.base))
            .unwrap()
            .unwrap()
    }

    fn plan_with(&self, online: bool, options: Options) -> Result<Plan> {
        let mut session = self.session();
        plan(
            &self.paths,
            online.then_some(&mut session),
            "rimworld",
            Options {
                refresh: true,
                ..options
            },
            &Token::new(),
        )
    }

    fn pending(&self, online: bool, options: Options) -> Pending {
        match self.plan_with(online, options).unwrap() {
            Plan::Update(pending) => pending,
            Plan::UpToDate { .. } => panic!("nothing to update"),
        }
    }

    fn apply_with(
        &self,
        online: bool,
        pending: &Pending,
        token: &Token,
        progress: impl FnMut(Progress),
    ) -> Result<Updated> {
        let mut session = self.session();
        apply(
            &self.paths,
            online.then_some(&mut session),
            pending,
            token,
            progress,
        )
    }

    fn try_update(&self, options: Options) -> Result<Updated> {
        let pending = match self.plan_with(true, options)? {
            Plan::Update(pending) => pending,
            Plan::UpToDate { .. } => panic!("nothing to update"),
        };
        self.apply_with(true, &pending, &Token::new(), |_| {})
    }

    fn update(&self) -> Updated {
        self.try_update(Options::default()).unwrap()
    }

    fn reinstall(&self) -> Updated {
        self.try_update(Options {
            reinstall: true,
            ..Options::default()
        })
        .unwrap()
    }

    /// An update that stops at an injected failpoint.
    fn update_err(&self) {
        let error = self.try_update(Options::default()).unwrap_err();
        assert!(format!("{error:#}").contains("injected"), "{error:#}");
    }

    fn write(&self, path: &str, content: &str) {
        let path = self.folder.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.folder.join(path)).unwrap()
    }

    /// The game folder as a map of path to what it holds.
    fn tree(&self) -> BTreeMap<String, String> {
        let mut tree = BTreeMap::new();
        walk(&self.folder, "", &mut tree);
        tree
    }

    fn temporaries(&self) -> Vec<String> {
        self.tree()
            .into_keys()
            .filter(|path| path.rsplit('/').next().unwrap().starts_with(".wineo-"))
            .collect()
    }

    fn clear_cache(&self) {
        let _ = fs::remove_dir_all(self.paths.gog_download_cache());
    }
}

fn walk(dir: &Path, prefix: &str, tree: &mut BTreeMap<String, String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let meta = fs::symlink_metadata(entry.path()).unwrap();
        if meta.file_type().is_symlink() {
            let target = fs::read_link(entry.path()).unwrap();
            tree.insert(path, format!("-> {}", target.display()));
        } else if meta.is_dir() {
            tree.insert(path.clone(), "/".to_owned());
            walk(&entry.path(), &path, tree);
        } else {
            let mode = meta.permissions().mode() & 0o777;
            let content = String::from_utf8_lossy(&fs::read(entry.path()).unwrap()).into_owned();
            tree.insert(path, format!("{mode:o} {content}"));
        }
    }
}

fn base_v1() -> Vec<Item> {
    vec![
        Item::Exe("start.sh", b"v1 start".to_vec()),
        file("gameinfo", "RimWorld\n1.0\n"),
        file("Data/core.dat", "core v1"),
        file("Data/old.dat", "old"),
        file("Data/old2.dat", "old2"),
        Item::Link("lib", "Data"),
        Item::Link("Manual", "docs/readme.txt"),
        file("docs/readme.txt", "readme"),
        file("shared.txt", "base shared"),
        Item::Split("Data/big.dat", b"big v1 big v1 big v1".to_vec()),
        file("old/deep/readme", "an obsolete folder"),
        file("kept/readme", "dropped from a folder you use"),
        file("notes/readme.txt", "a folder that becomes a file"),
        file("notes/deep/readme", "inside a folder that becomes a file"),
    ]
}

fn base_v2() -> Vec<Item> {
    vec![
        Item::Exe("start.sh", b"v2 start".to_vec()),
        file("gameinfo", "RimWorld\n2.0\n"),
        file("Data/core.dat", "core v2"),
        file("Data/new.dat", "new"),
        file("lib", "lib is a file now"),
        file("Manual/index.html", "manual"),
        file("docs/readme.txt", "readme"),
        file("shared.txt", "base shared"),
        Item::Split("Data/big.dat", b"big v2 big v2 big v2".to_vec()),
        file("notes", "notes is a file now"),
    ]
}

fn royalty_v1() -> Vec<Item> {
    vec![
        file("Data/Royalty/r.dat", "royalty v1"),
        file("shared.txt", "dlc shared"),
    ]
}

fn royalty_v2() -> Vec<Item> {
    vec![file("Data/Royalty/r.dat", "royalty v2")]
}

/// A game installed at version 1 with Royalty, every file recorded.
fn installed_v1() -> Fixture {
    let fx = Fixture::new();
    fx.offer(RIMWORLD, "RimWorld", "1.0", &base_v1());
    fx.offer(ROYALTY, "Royalty", "1.0", &royalty_v1());
    fx.migrated(&[(RIMWORLD, Some("1.0")), (ROYALTY, None)]);
    fx.reinstall();
    fx
}

/// Version 2 on GOG, and the user's own changes in the folder.
fn ready_for_v2(fx: &Fixture) {
    fx.offer(RIMWORLD, "RimWorld", "2.0", &base_v2());
    fx.offer(ROYALTY, "Royalty", "2.0", &royalty_v2());
    fx.write("mods-1.5/mod.txt", "my mod");
    fx.write("Data/core.dat", "my edit");
    fx.write("Data/old2.dat", "my old2");
    fx.write("Data/new.dat", "mine");
    fx.write("kept/user.txt", "mine too");
    fx.write("notes/mine.txt", "my notes");
    fx.write("notes/deep/mine.txt", "my deep notes");
}

fn expected_v2() -> BTreeMap<String, String> {
    [
        ("Data", "/"),
        ("Data/Royalty", "/"),
        ("Data/Royalty/r.dat", "644 royalty v2"),
        ("Data/big.dat", "644 big v2 big v2 big v2"),
        ("Data/core.dat", "644 core v2"),
        ("Data/new.dat", "644 new"),
        ("Data/new.dat.wineo-saved", "644 mine"),
        ("Data/old2.dat", "644 my old2"),
        ("Manual", "/"),
        ("Manual/index.html", "644 manual"),
        ("docs", "/"),
        ("docs/readme.txt", "644 readme"),
        ("gameinfo", "644 RimWorld\n2.0\n"),
        ("kept", "/"),
        ("kept/user.txt", "644 mine too"),
        ("notes", "644 notes is a file now"),
        ("notes.wineo-saved", "/"),
        ("notes.wineo-saved/deep", "/"),
        ("notes.wineo-saved/deep/mine.txt", "644 my deep notes"),
        ("notes.wineo-saved/mine.txt", "644 my notes"),
        ("lib", "644 lib is a file now"),
        ("mods-1.5", "/"),
        ("mods-1.5/mod.txt", "644 my mod"),
        ("shared.txt", "644 base shared"),
        ("start.sh", "755 v2 start"),
        ("support", "/"),
        (
            "support/postinst.sh",
            "755 #!/bin/bash\n# a stand-in for GOG's split file script\n",
        ),
        ("support/split_files", "644 Data/big.dat"),
    ]
    .into_iter()
    .map(|(path, content)| (path.to_owned(), content.to_owned()))
    .collect()
}

fn assert_finished_v2(fx: &Fixture) {
    assert_eq!(fx.tree(), expected_v2());
    let state = fx.state();
    assert_eq!(state.journal, None);
    let committed = state.committed.unwrap();
    assert_eq!(
        committed
            .products
            .iter()
            .map(|product| (product.id, product.version.as_deref()))
            .collect::<Vec<_>>(),
        [(RIMWORLD, Some("2.0")), (ROYALTY, Some("2.0"))]
    );
    let files: BTreeMap<&str, &WinningEntry> = committed
        .files
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    assert_eq!(files["Data/Royalty/r.dat"].product, ROYALTY);
    assert_eq!(files["shared.txt"].product, RIMWORLD);
    assert_eq!(
        files["Data/big.dat"].md5.as_deref(),
        Some(md5_hex(b"big v2 big v2 big v2").as_str())
    );
    assert!(!files.contains_key("Data/old.dat"));
    assert!(!files.contains_key("mods-1.5"));
    assert!(!files.keys().any(|path| path.contains(".split")));
}

#[test]
fn an_update_rebuilds_every_product_and_reports_what_was_yours() {
    let fx = installed_v1();
    assert_eq!(fx.read("shared.txt"), "dlc shared");
    assert_eq!(fx.read("Data/big.dat"), "big v1 big v1 big v1");
    assert_eq!(
        fx.state().committed.unwrap().products[1].version.as_deref(),
        Some("1.0")
    );
    ready_for_v2(&fx);

    let pending = fx.pending(true, Options::default());
    assert_eq!(pending.kind(), JournalKind::Update);
    assert!(!pending.is_recovery() && !pending.is_offline() && !pending.claims());
    let sizes: Vec<u64> = pending.products().iter().map(|p| p.download_size).collect();
    assert_eq!(pending.download_size(), sizes.iter().sum::<u64>());
    assert_eq!(pending.unpacked_estimate(), pending.download_size());
    assert_eq!(pending.products()[0].installed.as_deref(), Some("1.0"));
    assert_eq!(pending.products()[1].version.as_deref(), Some("2.0"));
    // Planning changes nothing.
    assert!(fx.state().journal.is_none());

    take_syncs();
    let mut events = Vec::new();
    let updated = fx
        .apply_with(true, &pending, &Token::new(), |event| events.push(event))
        .unwrap();
    assert_finished_v2(&fx);
    assert_eq!(
        updated.saved,
        [
            SavedAside {
                path: "notes".to_owned(),
                saved_as: "notes.wineo-saved".to_owned(),
            },
            SavedAside {
                path: "Data/new.dat".to_owned(),
                saved_as: "Data/new.dat.wineo-saved".to_owned(),
            }
        ]
    );
    assert_eq!(updated.kept, ["Data/old2.dat"]);
    assert_eq!(updated.overwritten, ["Data/core.dat"]);
    assert!(updated.warnings.is_empty(), "{:?}", updated.warnings);
    assert!(!updated.recovered && !updated.claimed);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Progress::Download { .. }))
    );
    assert!(events.iter().any(|e| matches!(e, Progress::Stage { .. })));
    assert!(events.iter().any(|e| matches!(e, Progress::Publish { .. })));
    // The packages are gone from the cache once committed.
    assert!(
        DownloadCache::new(fx.paths.gog_download_cache())
            .files(RIMWORLD)
            .unwrap()
            .is_empty()
    );

    // Every file and changed folder is synced before the commit.
    let syncs = take_syncs();
    let commit = syncs.iter().position(|event| event == "commit").unwrap();
    for path in [
        "start.sh",
        "Data/core.dat",
        "Data/new.dat",
        "lib",
        "Manual/index.html",
    ] {
        let at = syncs
            .iter()
            .position(|event| *event == format!("file:{path}"));
        assert!(
            at.is_some_and(|at| at < commit),
            "{path} synced after commit"
        );
    }
    for dir in ["", "Data", "Manual", "Data/Royalty"] {
        let at = syncs
            .iter()
            .position(|event| *event == format!("dir:{dir}"));
        assert!(
            at.is_some_and(|at| at < commit),
            "{dir:?} synced after commit"
        );
    }
}

#[test]
fn up_to_date_is_a_no_op_and_cant_compare_refuses() {
    let fx = installed_v1();
    let before = fx.state();
    match fx.plan_with(true, Options::default()).unwrap() {
        Plan::UpToDate {
            environment,
            version,
        } => {
            assert_eq!(environment, "rimworld");
            assert_eq!(version.as_deref(), Some("1.0"));
        }
        Plan::Update(_) => panic!("expected up to date"),
    }
    fx.withdraw(RIMWORLD);
    let error = fx.plan_with(true, Options::default()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "rimworld can't be updated: RimWorld has no Linux installer in en-US."
    );
    assert!(matches!(
        error.downcast_ref(),
        Some(UpdateError::CantCompare { .. })
    ));
    assert_eq!(fx.state(), before);
}

#[test]
fn a_same_version_reinstall_repairs_changed_and_missing_files() {
    let fx = installed_v1();
    fs::remove_file(fx.folder.join("Data/core.dat")).unwrap();
    fx.write("docs/readme.txt", "scribbled");
    let downloads = fx.downloads();
    let updated = fx.reinstall();
    assert_eq!(updated.kind, JournalKind::Reinstall);
    assert_eq!(fx.read("Data/core.dat"), "core v1");
    assert_eq!(fx.read("docs/readme.txt"), "readme");
    assert_eq!(updated.overwritten, ["docs/readme.txt"]);
    assert_eq!(fx.downloads(), downloads + 2);
}

/// Every path a folder's state records: committed winning entries and the
/// journal's `attempted`.
fn recorded_paths(state: &FolderState) -> Vec<String> {
    let committed = state.committed.iter().flat_map(|c| &c.files);
    let attempted = state.journal.iter().flat_map(|j| &j.attempted);
    committed
        .map(|entry| entry.path.clone())
        .chain(attempted.map(|entry| entry.path.clone()))
        .collect()
}

/// Checks that the run that committed synced, before its commit, every
/// folder that still exists and holds a path recorded before or after it,
/// which covers every folder any attempt changed.
fn assert_synced(fx: &Fixture, before: &FolderState, syncs: &[String]) {
    let commit = syncs
        .iter()
        .rposition(|event| event == "commit")
        .unwrap_or_else(|| panic!("no commit in {syncs:?}"));
    let synced = |dir: &str| {
        syncs
            .iter()
            .position(|event| *event == format!("dir:{dir}"))
            .is_some_and(|at| at < commit)
    };
    let mut paths = recorded_paths(before);
    paths.extend(recorded_paths(&fx.state()));
    let tree = fx.tree();
    let is_dir = |dir: &str| dir.is_empty() || tree.get(dir).is_some_and(|kind| kind == "/");
    for path in paths {
        let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
        if is_dir(parent) {
            assert!(
                synced(parent),
                "{parent:?} wasn't synced before the commit: {syncs:?}"
            );
            continue;
        }
        // A folder moved aside, or inside one, carries the changes made in
        // it: find it under every name an ancestor was saved as.
        let mut found = Vec::new();
        let mut ancestor = parent;
        loop {
            let rest = &parent[ancestor.len()..];
            for saved in tree.keys().filter(|saved| {
                saved.strip_prefix(ancestor).is_some_and(|suffix| {
                    suffix.starts_with(".wineo-saved") && !suffix.contains('/')
                })
            }) {
                let moved = format!("{saved}{rest}");
                if is_dir(&moved) {
                    found.push(moved);
                }
            }
            match ancestor.rsplit_once('/') {
                Some((up, _)) => ancestor = up,
                None => break,
            }
        }
        for moved in &found {
            assert!(
                synced(moved),
                "{moved:?} wasn't synced before the commit: {syncs:?}"
            );
        }
        // Found nowhere, here or under any name an ancestor was saved as,
        // means the operation removed it, and its parent's sync covers that.
    }
}

/// Runs the v1 to v2 update with a crash at the `skip`th pass of `point`,
/// then recovers, first crashing recovery at `again` when given. Returns
/// false once the point is never reached.
fn crash_and_recover(point: &'static str, skip: usize, again: Option<&'static str>) -> bool {
    let fx = installed_v1();
    ready_for_v2(&fx);
    let installed = fx.state();
    set_failpoint_nth(point, skip);
    take_syncs();
    let result = fx.try_update(Options::default());
    if failpoint_armed() {
        set_failpoint(None);
        result.unwrap();
        assert_finished_v2(&fx);
        assert_synced(&fx, &installed, &take_syncs());
        return false;
    }
    let error = result.unwrap_err();
    assert!(
        format!("{error:#}").contains("injected"),
        "{point}#{skip}: {error:#}"
    );
    // What the interrupted attempts made durable counts too.
    let mut syncs = take_syncs();
    if let Some(again) = again {
        assert!(fx.state().journal.is_some(), "{point}#{skip}");
        set_failpoint(Some(again));
        let pending = fx.pending(true, Options::default());
        assert!(pending.is_recovery());
        let error = fx
            .apply_with(true, &pending, &Token::new(), |_| {})
            .unwrap_err();
        assert!(!failpoint_armed(), "recovery never reached {again}");
        assert!(
            format!("{error:#}").contains("injected"),
            "{again}: {error:#}"
        );
        assert!(fx.state().journal.is_some(), "{again}");
        syncs.extend(take_syncs());
    }
    let before = fx.state();
    if before.journal.is_none()
        && before.committed.as_ref().unwrap().products[0]
            .version
            .as_deref()
            != Some("2.0")
    {
        // Stopped before the journal: nothing changed, so it's simply run
        // again.
        fx.update();
    } else if before.journal.is_some() {
        let pending = fx.pending(true, Options::default());
        assert!(pending.is_recovery(), "{point}#{skip}");
        assert_eq!(pending.kind(), JournalKind::Update);
        fx.apply_with(true, &pending, &Token::new(), |_| {})
            .unwrap_or_else(|error| panic!("{point}#{skip}: {error:#}"));
    }
    syncs.extend(take_syncs());
    assert_finished_v2(&fx);
    assert!(fx.temporaries().is_empty(), "{point}#{skip}");
    if syncs.iter().any(|event| event == "commit") {
        assert_synced(&fx, &installed, &syncs);
        assert_synced(&fx, &before, &syncs);
    }
    true
}

#[test]
fn a_crash_at_any_write_is_finished_by_recovery() {
    for point in [
        "update.journal.before",
        "update.journal.after",
        "update.attempted.before",
        "update.attempted.after",
        "update.publish.before",
        "update.publish.temp",
        "update.publish.after",
        "update.delete.before",
        "update.delete.after",
        "update.save.before",
        "update.save.after",
        "update.commit.before",
        "update.commit.after",
        "write.before",
        "write.before_rename",
        "write.after",
    ] {
        let mut skip = 0;
        while crash_and_recover(point, skip, None) {
            skip += 1;
        }
        assert!(skip > 0, "{point} was never reached");
    }
}

#[test]
fn a_recovery_that_crashes_is_recovered_again() {
    // The first attempt stops after one deletion, so recovery still has
    // deletions, publishing, and the commit ahead of it.
    for again in [
        "update.journal.before",
        "update.journal.after",
        "update.attempted.after",
        "update.delete.before",
        "update.delete.after",
        "update.save.after",
        "update.publish.after",
        "update.commit.before",
        "write.before_rename",
    ] {
        crash_and_recover("update.delete.after", 0, Some(again));
    }
    for again in ["update.publish.after", "update.commit.before"] {
        crash_and_recover("update.publish.after", 5, Some(again));
    }
}

#[test]
fn a_same_version_reinstall_crashing_around_its_commit() {
    let fx = installed_v1();
    let reinstall = Options {
        reinstall: true,
        ..Options::default()
    };
    set_failpoint(Some("update.commit.before"));
    fx.try_update(reinstall).unwrap_err();
    assert_eq!(fx.state().journal.unwrap().kind, JournalKind::Reinstall);
    // Versions match, but the journal's kind still rebuilds.
    let pending = fx.pending(true, Options::default());
    assert_eq!(pending.kind(), JournalKind::Reinstall);
    fx.apply_with(true, &pending, &Token::new(), |_| {})
        .unwrap();
    assert!(fx.state().journal.is_none());

    set_failpoint(Some("update.commit.after"));
    fx.try_update(reinstall).unwrap_err();
    assert!(fx.state().journal.is_none());
    assert!(matches!(
        fx.plan_with(true, Options::default()).unwrap(),
        Plan::UpToDate { .. }
    ));
}

#[test]
fn a_half_written_temporary_file_is_removed_and_never_yours() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    set_failpoint_nth("update.publish.temp", 3);
    fx.update_err();
    let operation = fx.state().journal.unwrap().operation;
    // What a crash partway through writing leaves.
    fx.write(&format!("Data/.wineo-{operation}-99"), "half");
    fx.write(&format!(".wineo-{operation}-stage-7/x"), "half");
    let updated = fx.update();
    assert!(updated.recovered);
    assert_finished_v2(&fx);
    assert!(!updated.kept.iter().any(|path| path.contains(".wineo-")));
    assert!(
        !updated
            .saved
            .iter()
            .any(|saved| saved.path.contains(".wineo-"))
    );
}

#[test]
fn a_file_only_a_failed_update_wrote_is_removed_by_a_newer_one() {
    let fx = installed_v1();
    let mut b = base_v1();
    b.push(file("Data/b_only.dat", "b"));
    fx.offer(RIMWORLD, "RimWorld", "1.5", &b);
    set_failpoint(Some("update.commit.before"));
    fx.update_err();
    assert_eq!(fx.read("Data/b_only.dat"), "b");

    // GOG moves on to C, which doesn't ship it, and B's packages are gone.
    fx.offer(RIMWORLD, "RimWorld", "2.0", &base_v1());
    fx.clear_cache();
    let pending = fx.pending(true, Options::default());
    assert!(pending.is_recovery() && !pending.is_offline());
    assert_eq!(pending.target().products[0].version.as_deref(), Some("2.0"));
    let attempted = fx.state().journal.unwrap().attempted;
    assert!(
        attempted
            .iter()
            .any(|entry| entry.path == "Data/b_only.dat")
    );
    let updated = fx
        .apply_with(true, &pending, &Token::new(), |_| {})
        .unwrap();
    assert!(!fx.folder.join("Data/b_only.dat").exists());
    assert!(updated.kept.is_empty());
    let committed = fx.state().committed.unwrap();
    assert_eq!(committed.products[0].version.as_deref(), Some("2.0"));
}

#[test]
fn recovery_uses_cached_packages_offline() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    set_failpoint_nth("update.publish.after", 4);
    fx.update_err();
    let downloads = fx.downloads();
    let pending = fx.pending(false, Options::default());
    assert!(pending.is_recovery() && pending.is_offline());
    assert_eq!(pending.download_size(), 0);
    fx.apply_with(false, &pending, &Token::new(), |_| {})
        .unwrap();
    assert_finished_v2(&fx);
    assert_eq!(fx.downloads(), downloads);
}

#[test]
fn an_update_interrupted_after_downloading_finishes_without_downloading_again() {
    // Stopped before the journal, its packages are found again from GOG's
    // rounded listing; after it, from the journal's exact sizes.
    for (point, skip) in [("update.journal.before", 0), ("update.publish.after", 2)] {
        let fx = installed_v1();
        ready_for_v2(&fx);
        fx.pad(RIMWORLD);
        fx.pad(ROYALTY);
        set_failpoint_nth(point, skip);
        fx.update_err();
        let downloads = fx.downloads();
        if let Some(journal) = fx.state().journal {
            let sizes: Vec<(u64, u64)> = journal
                .packages
                .iter()
                .map(|package| (package.product, package.size))
                .collect();
            assert_eq!(
                sizes,
                [
                    (RIMWORLD, fx.package_size(RIMWORLD)),
                    (ROYALTY, fx.package_size(ROYALTY))
                ]
            );
            let pending = fx.pending(false, Options::default());
            assert!(pending.is_offline(), "{point}");
            fx.apply_with(false, &pending, &Token::new(), |_| {})
                .unwrap();
        } else {
            fx.update();
        }
        assert_finished_v2(&fx);
        assert_eq!(fx.downloads(), downloads, "{point}");
    }
}

#[test]
fn recovery_with_packages_missing_needs_gog_and_keeps_the_journal() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    set_failpoint_nth("update.publish.after", 4);
    fx.update_err();
    fx.clear_cache();
    let journal = fx.state().journal.unwrap();
    let error = fx.plan_with(false, Options::default()).unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(AccountError::LoggedOut)
    ));
    assert_eq!(fx.state().journal.unwrap(), journal);

    // Online, the packages are resolved again, keeping what was attempted.
    let pending = fx.pending(true, Options::default());
    assert!(pending.is_recovery() && !pending.is_offline());
    fx.apply_with(true, &pending, &Token::new(), |_| {})
        .unwrap();
    assert_finished_v2(&fx);
}

#[test]
fn a_cached_package_that_no_longer_verifies_is_downloaded_again() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    set_failpoint(Some("update.attempted.before"));
    fx.update_err();
    let cache = DownloadCache::new(fx.paths.gog_download_cache());
    let cached = cache.files(RIMWORLD).unwrap();
    let mut bytes = fs::read(&cached[0].path).unwrap();
    bytes[0] ^= 1;
    fs::write(&cached[0].path, bytes).unwrap();
    let pending = fx.pending(true, Options::default());
    assert!(!pending.is_offline());
    fx.apply_with(true, &pending, &Token::new(), |_| {})
        .unwrap();
    assert_finished_v2(&fx);
}

#[test]
fn symlinks_are_replaced_never_written_through() {
    let fx = installed_v1();
    let outside = fx.root.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("core.dat"), "outside core").unwrap();
    fs::write(outside.join("start"), "outside start").unwrap();
    // The user swaps a shipped folder and a shipped file for symlinks out.
    fs::rename(fx.folder.join("Data"), fx.root.path().join("Data-moved")).unwrap();
    symlink(&outside, fx.folder.join("Data")).unwrap();
    fs::remove_file(fx.folder.join("start.sh")).unwrap();
    symlink(outside.join("start"), fx.folder.join("start.sh")).unwrap();
    fx.offer(RIMWORLD, "RimWorld", "2.0", &base_v2());
    fx.offer(ROYALTY, "Royalty", "2.0", &royalty_v2());
    let updated = fx.update();

    assert_eq!(
        fs::read_to_string(outside.join("core.dat")).unwrap(),
        "outside core"
    );
    assert_eq!(
        fs::read_to_string(outside.join("start")).unwrap(),
        "outside start"
    );
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 2);
    let tree = fx.tree();
    assert_eq!(tree["Data"], "/");
    assert_eq!(tree["Data/core.dat"], "644 core v2");
    assert_eq!(tree["start.sh"], "755 v2 start");
    assert!(tree["Data.wineo-saved"].starts_with("-> "));
    assert!(tree["start.sh.wineo-saved"].starts_with("-> "));
    let mut saved: Vec<&str> = updated.saved.iter().map(|s| s.path.as_str()).collect();
    saved.sort_unstable();
    assert_eq!(saved, ["Data", "start.sh"]);
    // A tracked symlink became a file and another a folder.
    assert_eq!(tree["lib"], "644 lib is a file now");
    assert_eq!(tree["Manual"], "/");
}

#[test]
fn a_changed_tracked_symlink_is_saved_aside_when_its_type_changes() {
    let fx = installed_v1();
    fs::remove_file(fx.folder.join("lib")).unwrap();
    symlink("elsewhere", fx.folder.join("lib")).unwrap();
    fx.offer(RIMWORLD, "RimWorld", "2.0", &base_v2());
    fx.offer(ROYALTY, "Royalty", "2.0", &royalty_v2());
    let updated = fx.update();
    assert_eq!(fx.tree()["lib.wineo-saved"], "-> elsewhere");
    assert!(updated.saved.iter().any(|saved| saved.path == "lib"));
}

#[test]
fn a_tracked_folder_holding_your_files_is_saved_aside_when_it_becomes_a_file() {
    let fx = installed_v1();
    fx.write("docs/notes.txt", "my notes");
    let mut v2 = base_v2();
    v2.retain(|item| !matches!(item, Item::File("docs/readme.txt", _)));
    v2.retain(|item| {
        !matches!(
            item,
            Item::Link("Manual", _) | Item::File("Manual/index.html", _)
        )
    });
    v2.push(file("docs", "docs is a file"));
    fx.offer(RIMWORLD, "RimWorld", "2.0", &v2);
    fx.offer(ROYALTY, "Royalty", "2.0", &royalty_v2());
    let updated = fx.update();
    let tree = fx.tree();
    assert_eq!(tree["docs"], "644 docs is a file");
    assert_eq!(tree["docs.wineo-saved/notes.txt"], "644 my notes");
    // The shipped file inside it was wineo's, so it went first.
    assert!(!tree.contains_key("docs.wineo-saved/readme.txt"));
    assert!(updated.saved.iter().any(|saved| saved.path == "docs"));
}

#[test]
fn a_dlc_only_update_rebuilds_and_records_it() {
    let fx = installed_v1();
    fx.offer(
        ROYALTY,
        "Royalty",
        "1.1",
        &[file("Data/Royalty/r.dat", "royalty 1.1")],
    );
    let pending = fx.pending(true, Options::default());
    assert_eq!(pending.products()[0].version.as_deref(), Some("1.0"));
    fx.apply_with(true, &pending, &Token::new(), |_| {})
        .unwrap();
    assert_eq!(fx.read("Data/Royalty/r.dat"), "royalty 1.1");
    // Royalty no longer ships the shared file, so the base's is back.
    assert_eq!(fx.read("shared.txt"), "base shared");
    let committed = fx.state().committed.unwrap();
    assert_eq!(committed.products[1].version.as_deref(), Some("1.1"));
}

#[test]
fn indirect_overlaps_across_three_products() {
    let fx = Fixture::new();
    fx.offer(
        RIMWORLD,
        "RimWorld",
        "1",
        &[file("x", "base x"), file("start.sh", "s")],
    );
    fx.offer(
        ROYALTY,
        "Royalty",
        "1",
        &[file("x", "royalty x"), file("y", "royalty y")],
    );
    fx.offer(IDEOLOGY, "Ideology", "1", &[file("y", "ideology y")]);
    fx.migrated(&[(RIMWORLD, Some("0")), (ROYALTY, None), (IDEOLOGY, None)]);
    fx.update();
    assert_eq!(fx.read("x"), "royalty x");
    assert_eq!(fx.read("y"), "ideology y");

    fx.offer(ROYALTY, "Royalty", "2", &[file("z", "royalty z")]);
    let updated = fx.update();
    assert_eq!(fx.read("x"), "base x");
    assert_eq!(fx.read("y"), "ideology y");
    assert_eq!(fx.read("z"), "royalty z");
    assert!(updated.kept.is_empty() && updated.saved.is_empty());
    let committed = fx.state().committed.unwrap();
    let product = |path: &str| {
        committed
            .files
            .iter()
            .find(|entry| entry.path == path)
            .unwrap()
            .product
    };
    assert_eq!(
        (product("x"), product("y"), product("z")),
        (RIMWORLD, IDEOLOGY, ROYALTY)
    );
}

#[test]
fn packages_that_cant_be_layered_are_refused_before_any_change() {
    let fx = Fixture::new();
    fx.offer(RIMWORLD, "RimWorld", "1", &[file("a/x", "x")]);
    fx.offer(ROYALTY, "Royalty", "1", &[file("a", "a file")]);
    fx.migrated(&[(RIMWORLD, Some("0")), (ROYALTY, None)]);
    let before = fx.state();
    let error = fx.try_update(Options::default()).unwrap_err();
    assert!(
        matches!(error.downcast_ref(), Some(UpdateError::Layers { .. })),
        "{error:#}"
    );
    assert_eq!(fx.state(), before);
    assert!(fx.tree().is_empty());
}

#[test]
fn a_package_no_longer_offered_is_refused_before_any_change() {
    let fx = installed_v1();
    fx.offer(RIMWORLD, "RimWorld", "2.0", &base_v2());
    fx.withdraw(ROYALTY);
    let before = (fx.state(), fx.tree());
    let error = fx
        .try_update(Options {
            reinstall: true,
            ..Options::default()
        })
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "rimworld can't be updated: Royalty (1233017772) has no Linux installer in en-US."
    );
    assert_eq!((fx.state(), fx.tree()), before);
}

#[test]
fn a_migrated_games_first_update_records_everything_and_keeps_your_files() {
    let fx = Fixture::new();
    // What Heroic left: version 1 unpacked, and a mod folder.
    for (path, content) in [
        ("start.sh", "v1 start"),
        ("Data/core.dat", "core v1"),
        ("Data/old.dat", "old"),
        ("Data/Royalty/r.dat", "royalty v1"),
        ("mods-1.5/mod.txt", "my mod"),
    ] {
        fx.write(path, content);
    }
    fx.migrated(&[(RIMWORLD, Some("1.0")), (ROYALTY, None)]);
    fx.offer(RIMWORLD, "RimWorld", "2.0", &base_v2());
    fx.offer(ROYALTY, "Royalty", "2.0", &royalty_v2());
    let updated = fx.update();
    assert!(updated.saved.is_empty() && updated.kept.is_empty());
    assert_eq!(
        updated.overwritten,
        ["Data/Royalty/r.dat", "Data/core.dat", "start.sh"]
    );
    let tree = fx.tree();
    assert_eq!(tree["Data/core.dat"], "644 core v2");
    assert_eq!(tree["Data/Royalty/r.dat"], "644 royalty v2");
    // Never recorded, so never removed.
    assert_eq!(tree["Data/old.dat"], "644 old");
    assert_eq!(tree["mods-1.5/mod.txt"], "644 my mod");
    let committed = fx.state().committed.unwrap();
    assert_eq!(committed.products[1].version.as_deref(), Some("2.0"));
    assert!(
        committed
            .files
            .iter()
            .any(|entry| entry.path == "Data/Royalty/r.dat")
    );
    assert!(
        !committed
            .files
            .iter()
            .any(|entry| entry.path.starts_with("mods-1.5"))
    );
}

#[test]
fn a_clone_running_from_the_folder_refuses_the_update() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    let pending = fx.pending(true, Options::default());
    let lock = InUseLock::new(&fx.paths, &fx.folder).unwrap();
    let held = lock.try_shared("rimworld-copy").unwrap().unwrap();
    let error = fx
        .apply_with(true, &pending, &Token::new(), |_| {})
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "rimworld-copy is running from this game folder. Stop it before updating rimworld."
    );
    assert!(fx.state().journal.is_none());
    drop(held);
}

#[test]
fn a_clone_launched_while_downloading_runs_and_the_packages_wait_for_it() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    let before = (fx.state(), fx.tree());
    let pending = fx.pending(true, Options::default());
    let lock = InUseLock::new(&fx.paths, &fx.folder).unwrap();
    let held = RefCell::new(None);
    let error = fx
        .apply_with(true, &pending, &Token::new(), |event| {
            if matches!(event, Progress::Download { .. }) && held.borrow().is_none() {
                // Downloading holds no lock, so the launch isn't refused.
                *held.borrow_mut() = Some(lock.try_shared("rimworld-copy").unwrap().unwrap());
            }
        })
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "rimworld-copy is running from this game folder. Stop it before updating rimworld."
    );
    assert_eq!((fx.state(), fx.tree()), before);
    let cache = DownloadCache::new(fx.paths.gog_download_cache());
    assert!(
        cache
            .files(RIMWORLD)
            .unwrap()
            .iter()
            .all(|file| file.complete)
    );
    assert!(!cache.files(ROYALTY).unwrap().is_empty());

    drop(held);
    let downloads = fx.downloads();
    fx.update();
    assert_eq!(fx.downloads(), downloads);
    assert_finished_v2(&fx);
}

#[test]
fn removing_the_owner_mid_update_and_recovering_afterwards() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    set_failpoint_nth("update.publish.after", 2);
    fx.update_err();
    let error = crate::trash::remove(&fx.paths, "rimworld", false).unwrap_err();
    assert!(error.to_string().contains("mid-update"), "{error}");
    let entry =
        crate::trash::remove_with_force(&fx.paths, "rimworld", false, true, &Token::new(), |_| {})
            .unwrap();
    let state = fx.state();
    assert_eq!(state.owner, None);
    assert!(state.journal.is_some());
    assert!(fx.plan_with(true, Options::default()).is_err());

    crate::trash::restore(&fx.paths, &entry.id).unwrap();
    let updated = fx.update();
    assert!(updated.recovered);
    assert_finished_v2(&fx);
}

#[test]
fn canceling_a_download_changes_nothing() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    let before = (fx.state(), fx.tree());
    let pending = fx.pending(true, Options::default());
    let token = Token::new();
    let error = fx
        .apply_with(true, &pending, &token, |event| {
            if matches!(event, Progress::Download { .. }) {
                token.cancel();
            }
        })
        .unwrap_err();
    assert!(
        matches!(
            error.downcast_ref(),
            Some(UpdateError::Canceled { journal: false })
        ),
        "{error:#}"
    );
    assert_eq!((fx.state(), fx.tree()), before);
}

#[test]
fn canceling_while_publishing_keeps_the_journal_for_recovery() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    let pending = fx.pending(true, Options::default());
    let token = Token::new();
    let error = fx
        .apply_with(true, &pending, &token, |event| {
            if matches!(event, Progress::Publish { done: 3, .. }) {
                token.cancel();
            }
        })
        .unwrap_err();
    assert!(
        matches!(
            error.downcast_ref(),
            Some(UpdateError::Canceled { journal: true })
        ),
        "{error:#}"
    );
    assert!(fx.state().journal.is_some());
    let updated = fx.update();
    assert!(updated.recovered);
    assert_finished_v2(&fx);
}

#[test]
fn not_enough_space_is_refused_before_any_change() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    let before = (fx.state(), fx.tree());
    FREE.with(|free| free.set(Some(10)));
    let error = fx.try_update(Options::default()).unwrap_err();
    FREE.with(|free| free.set(None));
    assert!(
        error.to_string().starts_with("Not enough space: needs "),
        "{error}"
    );
    assert!(error.to_string().ends_with(" free on ~/Games."), "{error}");
    assert_eq!((fx.state(), fx.tree()), before);
}

fn windows(fx: &Fixture, track: Track) {
    fx.save(&FolderState {
        committed: Some(Committed {
            gog_id: RIMWORLD,
            platform: Platform::Windows,
            language: "en".to_owned(),
            track,
            products: vec![Product {
                id: RIMWORLD,
                version: Some("1".to_owned()),
                build: Some("5".to_owned()),
                language: None,
                status: ProductStatus::Installed,
            }],
            files: Vec::new(),
        }),
        ..FolderState::new(fx.folder.clone(), Some("rimworld".to_owned()))
    });
}

#[test]
fn windows_games_refuse_saying_updates_come_later() {
    let fx = Fixture::new();
    for (track, said) in [
        (
            Track::Installer,
            "Updating Windows installer games comes in a later release.",
        ),
        (
            Track::Galaxy,
            "Updating Galaxy builds comes in a later release.",
        ),
    ] {
        windows(&fx, track);
        let before = fx.state();
        for reinstall in [false, true] {
            let error = fx
                .plan_with(
                    true,
                    Options {
                        reinstall,
                        ..Options::default()
                    },
                )
                .unwrap_err();
            assert_eq!(error.to_string(), said);
        }
        assert_eq!(fx.state(), before);
    }
}

#[test]
fn a_journal_from_a_later_release_is_left_alone() {
    let fx = installed_v1();
    let mut state = fx.state();
    state.journal = Some(Journal {
        operation: "op".to_owned(),
        kind: JournalKind::Repair,
        target: Target {
            gog_id: RIMWORLD,
            platform: Platform::Linux,
            language: "en-US".to_owned(),
            track: Track::Installer,
            products: Vec::new(),
        },
        packages: Vec::new(),
        attempted: Vec::new(),
    });
    fx.save(&state);
    let error = fx.plan_with(true, Options::default()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "rimworld has an unfinished GOG repair, and finishing it comes in a later release."
    );
    assert_eq!(fx.state(), state);
}

#[test]
fn a_portable_root_is_refused() {
    let fx = Fixture::new();
    let home = fx.root.path().to_owned();
    let portable = Paths::select(
        move |key| (key == "HOME").then(|| home.clone().into_os_string()),
        Some(&fx.root.path().join("usb")),
    )
    .unwrap();
    let error = plan(
        &portable,
        None,
        "rimworld",
        Options::default(),
        &Token::new(),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "GOG games live in the host library. Run this without --portable."
    );
}

#[test]
fn a_package_without_a_checksum_is_checked_by_size_with_a_warning() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    fx.gog
        .lock()
        .unwrap()
        .offers
        .get_mut(&ROYALTY)
        .unwrap()
        .checksum = false;
    let updated = fx.update();
    assert_eq!(
        updated.warnings,
        [
            "GOG publishes no checksum for Royalty (1233017772)'s package, so only its size was checked."
        ]
    );
    assert_finished_v2(&fx);
}

/// The folder's state without an owner, as a crash between a trash
/// restore's config and state writes leaves it.
fn unowned(fx: &Fixture) {
    let mut state = fx.state();
    state.owner = None;
    fx.save(&state);
}

#[test]
fn an_update_claims_an_ownerless_folder_its_targets_run_from() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    unowned(&fx);
    let pending = fx.pending(true, Options::default());
    assert!(pending.claims());
    assert_eq!(fx.state().owner, None);
    let updated = fx
        .apply_with(true, &pending, &Token::new(), |_| {})
        .unwrap();
    assert!(updated.claimed);
    assert_eq!(fx.state().owner.as_deref(), Some("rimworld"));
    assert_finished_v2(&fx);
}

#[test]
fn a_status_check_reads_an_ownerless_folder_without_claiming_it() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    unowned(&fx);
    let before = fx.state();
    let mut session = fx.session();
    let status = crate::gog::update::check(&mut session, "rimworld", true).unwrap();
    assert_eq!(status.state, UpdateState::Available);
    assert_eq!(status.environment, "rimworld");
    let batch = crate::gog::update::check_all(&mut session, false).unwrap();
    assert_eq!(batch.statuses.len(), 1, "{:?}", batch.errors);
    assert_eq!(batch.statuses[0].environment, "rimworld");
    assert!(batch.errors.is_empty(), "{:?}", batch.errors);
    assert_eq!(fx.state(), before);
}

#[test]
fn two_ownerless_folders_of_the_game_are_refused() {
    let fx = installed_v1();
    unowned(&fx);
    let other = fx.folder.parent().unwrap().join("RimWorld-2");
    fs::create_dir_all(&other).unwrap();
    let mut second = fx.state();
    second.folder = other.clone();
    fx.save(&second);
    let path = fx.paths.environment_config("rimworld");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("{text}cwd = \"{}\"\n", other.display())).unwrap();
    let error = fx.plan_with(true, Options::default()).unwrap_err();
    assert!(
        matches!(
            error.downcast_ref(),
            Some(FolderError::SeveralFolders { folders, .. }) if folders.len() == 2
        ),
        "{error:#}"
    );
}

#[test]
fn a_launch_claiming_first_wins_the_race() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    unowned(&fx);
    let pending = fx.pending(true, Options::default());
    fx.config("rimworld-copy", &fx.folder.join("start.sh"));
    // The copy's launch claims the folder between planning and applying.
    let gog_id = RIMWORLD.to_string();
    let start = [fx.folder.join("start.sh")];
    drop(folder::protect(&fx.paths, "rimworld-copy", Some(&gog_id), &start, &|| false).unwrap());
    assert_eq!(fx.state().owner.as_deref(), Some("rimworld-copy"));
    let error = fx
        .apply_with(true, &pending, &Token::new(), |_| {})
        .unwrap_err();
    assert!(
        matches!(
            error.downcast_ref(),
            Some(FolderError::OwnedByOther { owner, .. }) if owner == "rimworld-copy"
        ),
        "{error:#}"
    );
    assert!(fx.state().journal.is_none());

    // The environment's own launch claiming it first is fine.
    unowned(&fx);
    let pending = fx.pending(true, Options::default());
    drop(folder::protect(&fx.paths, "rimworld", Some(&gog_id), &start, &|| false).unwrap());
    fx.apply_with(true, &pending, &Token::new(), |_| {})
        .unwrap();
    assert_finished_v2(&fx);
}

#[test]
fn failed_and_unavailable_products_are_carried_over_without_downloading() {
    let fx = Fixture::new();
    fx.offer(RIMWORLD, "RimWorld", "2", &[file("start.sh", "s")]);
    fx.offer(ROYALTY, "Royalty", "2", &[file("r", "r")]);
    fx.offer(IDEOLOGY, "Ideology", "2", &[file("i", "i")]);
    fx.migrated(&[(RIMWORLD, Some("1")), (ROYALTY, None), (IDEOLOGY, None)]);
    let mut state = fx.state();
    let products = &mut state.committed.as_mut().unwrap().products;
    products[1].status = ProductStatus::Failed;
    products[2].status = ProductStatus::Unavailable;
    fx.save(&state);
    let pending = fx.pending(true, Options::default());
    assert_eq!(pending.products()[1].download_size, 0);
    fx.apply_with(true, &pending, &Token::new(), |_| {})
        .unwrap();
    assert_eq!(fx.downloads(), 1);
    assert!(!fx.folder.join("r").exists() && !fx.folder.join("i").exists());
    let committed = fx.state().committed.unwrap();
    assert_eq!(committed.products[1].status, ProductStatus::Failed);
    assert_eq!(committed.products[2].status, ProductStatus::Unavailable);
    assert_eq!(committed.products[1].version, None);
    assert!(
        committed
            .files
            .iter()
            .all(|entry| entry.product == RIMWORLD)
    );
}

#[test]
fn an_obsolete_folder_and_one_that_becomes_a_file_are_removed() {
    let fx = Fixture::new();
    fx.offer(
        RIMWORLD,
        "RimWorld",
        "1",
        &[
            file("start.sh", "s"),
            file("old/readme", "old"),
            file("docs/deep/readme", "docs"),
        ],
    );
    fx.migrated(&[(RIMWORLD, Some("0"))]);
    fx.update();
    fx.offer(
        RIMWORLD,
        "RimWorld",
        "2",
        &[file("start.sh", "s"), file("docs", "docs is a file")],
    );
    let updated = fx.update();
    assert!(updated.saved.is_empty() && updated.kept.is_empty());
    assert_eq!(
        fx.tree(),
        BTreeMap::from([
            ("docs".to_owned(), "644 docs is a file".to_owned()),
            ("start.sh".to_owned(), "644 s".to_owned()),
        ])
    );
}

#[test]
fn recovery_syncs_folders_an_earlier_attempt_changed() {
    let fx = Fixture::new();
    fx.offer(
        RIMWORLD,
        "RimWorld",
        "1",
        &[file("start.sh", "s"), file("kept/readme", "old")],
    );
    fx.migrated(&[(RIMWORLD, Some("0"))]);
    fx.update();
    fx.write("kept/user.txt", "mine");
    fx.offer(RIMWORLD, "RimWorld", "2", &[file("start.sh", "s2")]);
    set_failpoint(Some("update.delete.after"));
    fx.update_err();
    assert!(!fx.folder.join("kept/readme").exists());

    // Recovery finds the deletion done, but it may not be on disk yet.
    take_syncs();
    fx.update();
    let syncs = take_syncs();
    let commit = syncs.iter().position(|event| event == "commit").unwrap();
    let synced = syncs.iter().position(|event| event == "dir:kept");
    assert!(synced.is_some_and(|at| at < commit), "{syncs:?}");
}

#[test]
fn a_changed_shipped_file_is_listed_even_when_it_matches_the_new_one() {
    let fx = installed_v1();
    fx.offer(RIMWORLD, "RimWorld", "2.0", &base_v2());
    fx.offer(ROYALTY, "Royalty", "2.0", &royalty_v2());
    fx.write("Data/core.dat", "core v2");
    let updated = fx.update();
    assert_eq!(updated.overwritten, ["Data/core.dat"]);
    assert_eq!(fx.read("Data/core.dat"), "core v2");
}

#[test]
fn a_migrated_games_first_rebuild_lists_files_that_differed() {
    let fx = Fixture::new();
    fx.write("start.sh", "s");
    fx.write("Data/core.dat", "core v1");
    fx.write("Data/same.dat", "same");
    fx.migrated(&[(RIMWORLD, Some("1"))]);
    fx.offer(
        RIMWORLD,
        "RimWorld",
        "2",
        &[
            file("start.sh", "s"),
            file("Data/core.dat", "core v2"),
            file("Data/same.dat", "same"),
        ],
    );
    let updated = fx.update();
    assert_eq!(updated.overwritten, ["Data/core.dat"]);
    assert!(updated.saved.is_empty());
    assert_eq!(fx.read("Data/core.dat"), "core v2");
}

fn cached(fx: &Fixture, product: u64) -> usize {
    DownloadCache::new(fx.paths.gog_download_cache())
        .files(product)
        .unwrap()
        .len()
}

#[test]
fn an_update_pins_its_packages_until_its_journal_names_them() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    let paths = fx.paths.clone();
    let checked = std::rc::Rc::new(Cell::new(false));
    let seen = std::rc::Rc::clone(&checked);
    BEFORE_JOURNAL.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let cache = DownloadCache::new(paths.gog_download_cache());
            for product in [RIMWORLD, ROYALTY] {
                let error = cache.pin(product, &|| true).unwrap_err();
                assert_eq!(error.kind(), wineo_gog::ErrorKind::Canceled);
            }
            seen.set(true);
        }))
    });
    set_failpoint(Some("update.journal.after"));
    fx.update_err();
    assert!(checked.get());
    // Once the journal names them, they're free again, and kept.
    let cache = DownloadCache::new(fx.paths.gog_download_cache());
    drop(cache.pin(RIMWORLD, &|| true).unwrap());
    assert_eq!(cached(&fx, RIMWORLD), 1);
}

fn update_journal(fx: &Fixture) {
    let mut state = fx.state();
    state.journal = Some(Journal {
        operation: "op".to_owned(),
        kind: JournalKind::Update,
        target: Target {
            gog_id: RIMWORLD,
            platform: Platform::Linux,
            language: "en-US".to_owned(),
            track: Track::Installer,
            products: Vec::new(),
        },
        packages: vec![Package {
            product: RIMWORLD,
            download_id: "installer_linux_en".to_owned(),
            file_id: "en3installer0".to_owned(),
            size: 1,
            md5: None,
        }],
        attempted: Vec::new(),
    });
    fx.save(&state);
}

#[test]
fn cleanup_keeps_packages_a_journal_names_while_it_waits() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    // An update stopped before its journal: its packages are cached and
    // nothing names them yet.
    set_failpoint(Some("update.journal.before"));
    fx.update_err();
    assert_eq!(cached(&fx, RIMWORLD), 1);

    // Another update holds them from verifying until its journal is
    // written, while this one's cleanup tries to remove them.
    let cache = DownloadCache::new(fx.paths.gog_download_cache());
    let pin = cache.pin(RIMWORLD, &|| false).unwrap();
    std::thread::scope(|scope| {
        let (steps, step) = std::sync::mpsc::channel();
        let (go, cleanup) = cleanup_in_steps(scope, fx.paths.clone(), RIMWORLD, steps);
        assert_eq!(step.recv().unwrap(), "pinning");
        go.send(()).unwrap();
        update_journal(&fx);
        drop(pin);
        assert_eq!(step.recv().unwrap(), "decided");
        go.send(()).unwrap();
        assert!(cleanup.join().unwrap().is_empty());
    });
    assert_eq!(cached(&fx, RIMWORLD), 1);
}

#[test]
fn cleanup_decides_and_removes_without_letting_an_update_pin_in_between() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    set_failpoint(Some("update.journal.before"));
    fx.update_err();
    let cache = DownloadCache::new(fx.paths.gog_download_cache());
    std::thread::scope(|scope| {
        let (steps, step) = std::sync::mpsc::channel();
        let (go, cleanup) = cleanup_in_steps(scope, fx.paths.clone(), RIMWORLD, steps);
        assert_eq!(step.recv().unwrap(), "pinning");
        go.send(()).unwrap();
        // Nothing named the packages when cleanup looked, and until it has
        // removed them no update can pin them to write a journal.
        assert_eq!(step.recv().unwrap(), "decided");
        let error = cache.pin(RIMWORLD, &|| true).unwrap_err();
        assert_eq!(error.kind(), wineo_gog::ErrorKind::Canceled);
        go.send(()).unwrap();
        assert!(cleanup.join().unwrap().is_empty());
    });
    assert_eq!(cached(&fx, RIMWORLD), 0);
}

#[test]
fn cleanup_keeps_packages_while_a_state_cant_be_read() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    set_failpoint(Some("update.journal.before"));
    fx.update_err();
    fs::write(fx.paths.gog_folders_dir().join("broken.toml"), "not [ toml").unwrap();
    let mut warnings = Vec::new();
    remove_packages(&fx.paths, &fx.folder, &[RIMWORLD], &mut warnings);
    assert_eq!(cached(&fx, RIMWORLD), 1);
    assert!(
        warnings[0]
            .starts_with("Kept the downloaded GOG packages in case another game needs them: "),
        "{warnings:?}"
    );
}

#[test]
fn the_changed_message_names_the_command_to_run() {
    let error = UpdateError::Changed {
        name: "rimworld".to_owned(),
    };
    assert_eq!(
        error.to_string(),
        "rimworld's GOG game changed while updating; run wineo update rimworld again."
    );
}

#[test]
fn a_read_only_plan_with_refresh_reviews_what_gog_offers_now() {
    let fx = installed_v1();
    let cache = fx.paths.gog_product_cache(RIMWORLD);
    let cached = fs::read(&cache).unwrap();
    ready_for_v2(&fx);
    let pending = fx.pending(
        true,
        Options {
            read_only: true,
            ..Options::default()
        },
    );
    let versions: Vec<_> = pending
        .products()
        .iter()
        .map(|p| p.version.clone())
        .collect();
    assert_eq!(versions, [Some("2.0".to_owned()), Some("2.0".to_owned())]);
    // Nothing was cached for the next run either.
    assert_eq!(fs::read(&cache).unwrap(), cached);
}

fn write_info(fx: &Fixture, id: u64, name: &str) {
    fx.write(
        &format!("game/goggame-{id}.info"),
        &format!(r#"{{"gameId":"{id}","name":"{name}"}}"#),
    );
}

#[test]
fn a_dlc_is_labeled_by_gog_s_title_before_the_folder_s_name() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    write_info(&fx, RIMWORLD, "RimWorld");
    write_info(&fx, ROYALTY, "RimWorld - Royalty Edition");
    let pending = fx.pending(true, Options::default());
    assert_eq!(pending.products()[1].title.as_deref(), Some("Royalty"));
    assert_eq!(
        pending.products()[1].name.as_deref(),
        Some("RimWorld - Royalty Edition")
    );
    assert_eq!(pending.dlc_label(ROYALTY), format!("Royalty ({ROYALTY})"));
}

#[test]
fn a_recovery_labels_a_dlc_by_the_folder_s_name_or_its_id() {
    let fx = installed_v1();
    ready_for_v2(&fx);
    set_failpoint_nth("update.publish.after", 4);
    fx.update_err();
    let pending = fx.pending(false, Options::default());
    assert!(pending.is_recovery());
    assert_eq!(pending.products()[1].title, None);
    assert_eq!(pending.dlc_label(ROYALTY), format!("DLC ({ROYALTY})"));

    write_info(&fx, RIMWORLD, "RimWorld");
    write_info(&fx, ROYALTY, "RimWorld - Royalty");
    let pending = fx.pending(false, Options::default());
    assert_eq!(pending.dlc_label(ROYALTY), format!("Royalty ({ROYALTY})"));
}

pub(in crate::gog::update) mod install;
