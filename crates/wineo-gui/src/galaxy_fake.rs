//! A stand-in for GOG with a content system, for the Galaxy tests: Papers,
//! Please owned and built in versions, a `galaxy` runner whose Wine runs
//! setup's batch files through the shared `cmd` stand-in, and a game
//! installed through core's own Galaxy install.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};
use wineo_core::cancel::Token;
use wineo_core::gog::account::Session;
use wineo_core::gog::update::galaxy::install as galaxy_install;
use wineo_core::gog::update::install::Request;
use wineo_core::paths::Paths;
use wineo_gog::fake::content::{ContentSystem, FakeBuild, FakeDependency, FakeDepot, FakeFile};
use wineo_gog::fake::{self, FakeGog, Reply};

use crate::gog_fake::{USER, save_login, tokens};
use crate::settings::steam_key::tests::TestKeyring;
use crate::test_runners::FAKE_CMD;
use crate::test_script::write_script;

pub(crate) const GAME: u64 = 1207658924;
pub(crate) const NAME: &str = "papers-please";

/// Papers, Please at a build named `version` whose id is `build`: its
/// program and levels change from build to build, its core data doesn't.
pub(crate) fn papers(build: &str, version: &str) -> FakeBuild {
    let info = json!({
        "gameId": GAME.to_string(),
        "rootGameId": GAME.to_string(),
        "buildId": build,
        "name": "Papers, Please",
        "language": "English",
        "playTasks": [{
            "type": "FileTask", "isPrimary": true, "path": "Papers.exe", "category": "game",
        }],
        "version": 1,
    });
    let mut build = FakeBuild::new(GAME, build).depot(
        FakeDepot::new(GAME)
            .file(FakeFile::new(
                "Papers.exe",
                format!("MZ {version}").as_bytes(),
            ))
            .file(FakeFile::new("data\\core.dat", b"core data that never changes").with_md5())
            .file(FakeFile::new(
                "data\\levels.dat",
                format!("levels of {version}").as_bytes(),
            ))
            .file(FakeFile::new(
                &format!("goggame-{GAME}.info"),
                info.to_string().as_bytes(),
            )),
    );
    build.version_name = version.to_owned();
    build.dependency("MSVC2017")
}

pub(crate) struct Galaxy {
    pub(crate) root: tempfile::TempDir,
    pub(crate) paths: Paths,
    pub(crate) keyring: TestKeyring,
    pub(crate) content: Arc<ContentSystem>,
    newest_first: Arc<AtomicBool>,
    pub(crate) server: FakeGog,
}

impl Galaxy {
    /// A logged-in home, Papers, Please owned with build 100 (1.2.76)
    /// listed, and its runtime exiting with 0.
    pub(crate) fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
                .unwrap();
        let keyring = TestKeyring(std::cell::RefCell::new(None));
        save_login(&paths, &keyring);
        let content = Arc::new(ContentSystem::new().with_chunk_size(8));
        let newest_first = Arc::new(AtomicBool::new(false));
        let server = {
            let (content, newest_first) = (Arc::clone(&content), Arc::clone(&newest_first));
            FakeGog::start(move |request| answer(&content, &newest_first, request))
        };
        content.add_build(papers("100", "1.2.76"));
        let world = Self {
            root,
            paths,
            keyring,
            content,
            newest_first,
            server,
        };
        world.runtime_exits("0");
        world.runner();
        world
    }

    /// The runtime setup runs, ending with `code`.
    pub(crate) fn runtime_exits(&self, code: &str) {
        self.runtime(b"vc", code);
    }

    /// The runtime as `program`, which setup runs again when it differs
    /// from the one installed.
    pub(crate) fn runtime(&self, program: &[u8], code: &str) {
        self.content.set_dependencies(
            "100",
            &[FakeDependency::new(
                "MSVC2017",
                "__redist\\MSVC2017\\vc.exe",
                "/quiet",
                FakeDepot::new(0)
                    .file(FakeFile::new("__redist\\MSVC2017\\vc.exe", program))
                    .file(FakeFile::new(
                        "__redist\\MSVC2017\\vc.exe.exit",
                        code.as_bytes(),
                    )),
            )],
        );
    }

    /// Lists a newer build ahead of the ones listed.
    pub(crate) fn newer(&self, build: &str, version: &str) {
        self.newest_first.store(true, Ordering::Relaxed);
        self.content.add_build(papers(build, version));
    }

    /// A runner `galaxy` whose Wine builds a prefix and runs setup's batch
    /// files.
    fn runner(&self) {
        let bin = wineo_core::config::GlobalConfig::default()
            .runner_root(&self.paths)
            .unwrap()
            .join("galaxy/bin");
        fs::create_dir_all(&bin).unwrap();
        write_script(bin.join("wineserver"), "#!/bin/sh\nexit 0\n");
        write_script(
            bin.join("wine"),
            format!(
                r#"#!/bin/sh
FAKE_CMD_LOG='{log}'
{cmd}
case "$1" in
wineboot) [ "$2" = --init ] && mkdir -p "$WINEPREFIX/drive_c" && printf 'WINE REGISTRY Version 2\n\n#arch=win64\n' > "$WINEPREFIX/system.reg"; exit 0 ;;
--version) echo wine-galaxy; exit 0 ;;
cmd) case "$3" in *.bat) fake_cmd "$3"; exit ;; esac; exit 0 ;;
esac
exit 0
"#,
                log = self.root.path().join("cmd.log").display(),
                cmd = FAKE_CMD,
            ),
        );
    }

    pub(crate) fn folder(&self) -> PathBuf {
        fs::canonicalize(self.root.path())
            .unwrap()
            .join("Games")
            .join("Papers")
    }

    pub(crate) fn request(&self) -> Request {
        Request {
            name: NAME.to_owned(),
            gog_id: GAME,
            dir: Some(self.folder()),
            refresh: true,
            runner: Some("galaxy".to_owned()),
            ..Request::default()
        }
    }

    /// Installs the game through core, at the build listed last.
    pub(crate) fn install(&self) {
        wineo_core::gog::update::windows::set_graphical_session(Some(true));
        let mut session = Session::open(&self.paths, &self.keyring, self.server.client())
            .unwrap()
            .unwrap();
        let plan = galaxy_install::plan(&self.paths, &mut session, &self.request(), &Token::new())
            .unwrap();
        galaxy_install::apply(&self.paths, &mut session, &plan, &Token::new(), |_| {}).unwrap();
    }

    /// Where a file of the game is.
    pub(crate) fn file(&self, path: &str) -> PathBuf {
        self.folder().join(path)
    }
}

fn answer(content: &ContentSystem, newest_first: &AtomicBool, request: &fake::Request) -> Reply {
    if ContentSystem::cdn_of(request).is_some() {
        return content
            .answer(request)
            .unwrap_or_else(|| fake::json(404, "{}"));
    }
    let path = request.path();
    if path == "/token" {
        return Reply::Send(tokens("REFRESH", USER));
    }
    if path == "/user/data/games" {
        return fake::json(200, &json!({ "owned": [GAME] }).to_string());
    }
    if path == "/account/getFilteredProducts" {
        let owned = json!([{
            "id": GAME,
            "title": "Papers, Please",
            "worksOn": {"Windows": true, "Mac": false, "Linux": false},
            "dlcCount": 0,
        }]);
        return fake::json(
            200,
            &json!({"page": 1, "totalPages": 1, "products": owned}).to_string(),
        );
    }
    if path == format!("/products/{GAME}") {
        let details = json!({
            "id": GAME,
            "title": "Papers, Please",
            "content_system_compatibility": {"windows": true, "osx": false, "linux": false},
            "downloads": {"installers": [], "patches": []},
            "dlcs": {"products": []},
            "expanded_dlcs": [],
        });
        return fake::json(200, &details.to_string());
    }
    let answered = content.answer(request);
    if path.ends_with("/os/windows/builds")
        && newest_first.load(Ordering::Relaxed)
        && let Some(Reply::Send(bytes)) = &answered
    {
        let text = String::from_utf8_lossy(bytes);
        let body = &text[text.find("\r\n\r\n").unwrap() + 4..];
        let mut list: Value = serde_json::from_str(body).unwrap();
        list["items"].as_array_mut().unwrap().reverse();
        return fake::json(200, &list.to_string());
    }
    answered.unwrap_or_else(|| fake::json(404, "{}"))
}
