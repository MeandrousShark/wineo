//! Installs from the real GOG, for `cargo test -- --ignored`, since the fake
//! one can't show how real installers handle paths and languages.
//!
//! They need a login in `WINEO_TEST_GOG_LOGIN`, as the JSON wineo keeps in
//! the keyring (`{"refresh_token": "...", "user_id": "..."}`), and the ids of
//! small games it owns: `WINEO_TEST_GOG_NATIVE` with a Linux installer,
//! `WINEO_TEST_GOG_WINDOWS`, and `WINEO_TEST_GOG_LANGUAGES`, a Windows game
//! in several languages that installs in `WINEO_TEST_GOG_LANGUAGE` (default
//! `de-DE`). Windows installs run in the Proton build `WINEO_TEST_PROTON`
//! names (the directory holding `proton`, which needs umu-run), else in
//! system Wine, and need a graphical session. A test missing any of that
//! says so and passes. GOG may hand out a new refresh token when the login
//! is renewed, which the test keeps only in memory.

use std::os::unix::fs::symlink;

use super::*;
use crate::config::GlobalConfig;
use crate::gog::language;
use crate::gog::update::install::{self, Installed, Request};
use crate::gog::update::windows::read_info;
use crate::gog::update::windows::tests::set_graphical;

struct Real {
    _root: TempDir,
    paths: Paths,
    keyring: Arc<FakeKeyring>,
}

impl Real {
    fn new() -> Option<Self> {
        let Some(secret) = env("WINEO_TEST_GOG_LOGIN") else {
            eprintln!("skipping: set WINEO_TEST_GOG_LOGIN to a GOG login");
            return None;
        };
        let (root, paths) = setup();
        let keyring = Arc::new(FakeKeyring::default());
        crate::key_store::save_key(&account::KEY_SPEC, &paths, &*keyring, &secret).unwrap();
        Some(Self {
            _root: root,
            paths,
            keyring,
        })
    }

    fn session(&self) -> Session<'_> {
        Session::open(
            &self.paths,
            &*self.keyring,
            crate::gog::client_from_env().unwrap(),
        )
        .unwrap()
        .expect("WINEO_TEST_GOG_LOGIN is a login")
    }

    /// The runner a Windows game gets, when there is one and a graphical
    /// session to show its installer in.
    fn windows_runner(&self) -> Option<String> {
        if !crate::launch::graphical_session() {
            eprintln!("skipping: a Windows install needs a graphical session");
            return None;
        }
        set_graphical(true);
        if let Some(source) = env("WINEO_TEST_PROTON") {
            let source = PathBuf::from(source);
            let name = source.file_name()?.to_str()?.to_owned();
            let runners = GlobalConfig::default().runner_root(&self.paths).unwrap();
            fs::create_dir_all(&runners).unwrap();
            symlink(&source, runners.join(&name)).unwrap();
            return Some(name);
        }
        let on_path = std::env::var_os("PATH")
            .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("wine").is_file()));
        if !on_path {
            eprintln!("skipping: set WINEO_TEST_PROTON or install Wine");
            return None;
        }
        Some(crate::runner::Runner::SYSTEM.to_owned())
    }

    fn install(&self, request: &Request) -> Installed {
        let mut session = self.session();
        let plan = install::plan(&self.paths, &mut session, request, &Token::new()).unwrap();
        install::apply(&self.paths, &mut session, &plan, &Token::new(), |_| {}).unwrap()
    }

    fn committed(&self, installed: &Installed) -> Committed {
        let state = folder::peek(&self.paths, &installed.folder)
            .unwrap()
            .unwrap();
        assert_eq!(state.journal, None);
        state.committed.unwrap()
    }
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

fn game(key: &str) -> Option<u64> {
    let id = env(key).and_then(|id| id.parse().ok());
    if id.is_none() {
        eprintln!("skipping: set {key} to the id of a small GOG game you own");
    }
    id
}

#[test]
#[ignore = "installs a real game from GOG"]
fn a_real_native_game_installs() {
    let (Some(real), Some(id)) = (Real::new(), game("WINEO_TEST_GOG_NATIVE")) else {
        return;
    };
    let installed = real.install(&Request {
        name: "native".to_owned(),
        gog_id: id,
        platform: Some(Platform::Linux),
        refresh: true,
        ..Request::default()
    });
    let committed = real.committed(&installed);
    assert_eq!(committed.platform, Platform::Linux);
    assert!(!committed.files.is_empty());
    assert!(installed.folder.join("start.sh").is_file());
}

#[test]
#[ignore = "installs a real game from GOG and runs its Windows installer"]
fn a_real_windows_game_installs() {
    let (Some(real), Some(id)) = (Real::new(), game("WINEO_TEST_GOG_WINDOWS")) else {
        return;
    };
    let Some(runner) = real.windows_runner() else {
        return;
    };
    let installed = real.install(&Request {
        name: "windows".to_owned(),
        gog_id: id,
        platform: Some(Platform::Windows),
        runner: Some(runner),
        refresh: true,
        ..Request::default()
    });
    let committed = real.committed(&installed);
    assert_eq!(committed.platform, Platform::Windows);
    assert!(read_info(&installed.folder, id).is_some());
    assert!(installed.warnings.is_empty(), "{:?}", installed.warnings);
}

#[test]
#[ignore = "installs a real game from GOG in another language"]
fn a_real_windows_game_installs_in_another_language() {
    let (Some(real), Some(id)) = (Real::new(), game("WINEO_TEST_GOG_LANGUAGES")) else {
        return;
    };
    let Some(runner) = real.windows_runner() else {
        return;
    };
    let tag = env("WINEO_TEST_GOG_LANGUAGE").unwrap_or_else(|| "de-DE".to_owned());
    let code = language::installer_code(&tag).unwrap();
    let installed = real.install(&Request {
        name: "languages".to_owned(),
        gog_id: id,
        platform: Some(Platform::Windows),
        runner: Some(runner),
        language: Some(tag.clone()),
        refresh: true,
        ..Request::default()
    });
    assert_eq!(real.committed(&installed).language, code);
    // The installed files say which language it is in.
    let info = read_info(&installed.folder, id).expect("the game's info file");
    let installed_tag = info["languages"][0].as_str().unwrap();
    assert_eq!(
        language::installer_code(installed_tag).unwrap(),
        code,
        "asked for {tag}, the game's info file lists {installed_tag}"
    );
}
