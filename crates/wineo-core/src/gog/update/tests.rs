use std::collections::BTreeMap;
use std::ffi::OsString;
use std::sync::Arc;

use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;
use crate::gog::account;
use crate::gog::folder::{Journal, JournalKind, Target, set_failpoint};
use crate::gog::migrate::MigrateError;
use crate::gog::test_support::{FakeKeyring, Scripted, client_at, setup, tokens};
use crate::steam::api::fixture::reply;

const RIMWORLD: u64 = 1094900565;
const ROYALTY: u64 = 1233017772;
const IDEOLOGY: u64 = 1827857764;
const DISCO: u64 = 1771589310;
const USER: u64 = 1001;
const NEVER: &dyn Fn() -> bool = &|| false;

/// One installer in GOG's product details.
fn installer(os: &str, language: &str, version: Option<&str>) -> Value {
    let mut installer = json!({
        "id": format!("installer_{os}_{language}"),
        "os": os,
        "language": language,
        "total_size": 7,
        "files": [{"id": "f", "size": 7, "downlink": "https://api.gog.com/x"}],
    });
    if let Some(version) = version {
        installer["version"] = json!(version);
    }
    installer
}

fn linux(version: &str) -> Vec<Value> {
    vec![installer("linux", "en", Some(version))]
}

fn product(id: u64, title: &str, installers: Vec<Value>, dlcs: Vec<Value>) -> Value {
    json!({
        "id": id,
        "title": title,
        "downloads": {"installers": installers},
        "expanded_dlcs": dlcs,
    })
}

fn dlc(id: u64, title: &str, installers: Vec<Value>) -> Value {
    product(id, title, installers, Vec::new())
}

fn builds(items: &[(&str, Option<&str>)]) -> Value {
    json!({
        "items": items
            .iter()
            .map(|(id, branch)| json!({"build_id": id, "branch": branch}))
            .collect::<Vec<_>>(),
        "has_private_branches": false,
    })
}

fn installed(id: u64, version: Option<&str>) -> folder::Product {
    folder::Product {
        id,
        version: version.map(str::to_owned),
        build: None,
        status: ProductStatus::Installed,
    }
}

fn built(id: u64, build: Option<&str>) -> folder::Product {
    folder::Product {
        id,
        version: None,
        build: build.map(str::to_owned),
        status: ProductStatus::Installed,
    }
}

struct Fixture {
    root: TempDir,
    paths: Paths,
    keyring: Arc<FakeKeyring>,
    products: BTreeMap<u64, Value>,
    builds: BTreeMap<u64, Value>,
}

impl Fixture {
    fn new() -> Self {
        let (root, paths) = setup();
        let keyring = Arc::new(FakeKeyring::default());
        let secret = format!(r#"{{"refresh_token":"R","user_id":"{USER}"}}"#);
        crate::key_store::save_key(&account::KEY_SPEC, &paths, &*keyring, &secret).unwrap();
        Self {
            root,
            paths,
            keyring,
            products: BTreeMap::new(),
            builds: BTreeMap::new(),
        }
    }

    /// A game folder owned by an environment `name` that tracks it.
    fn game(
        &self,
        name: &str,
        gog_id: u64,
        platform: Platform,
        track: Track,
        language: &str,
        products: Vec<folder::Product>,
    ) -> FolderState {
        let folder = fs::canonicalize(self.root.path())
            .unwrap()
            .join("Games")
            .join(name);
        fs::create_dir_all(&folder).unwrap();
        let path = self.paths.environment_config(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!(
                "kind = \"native\"\ngog_id = \"{gog_id}\"\n\n[targets.game]\nexe = \"{}\"\n",
                folder.join("start.sh").display()
            ),
        )
        .unwrap();
        let state = FolderState {
            committed: Some(Committed {
                gog_id,
                platform,
                language: language.to_owned(),
                track,
                products,
                files: Vec::new(),
            }),
            ..FolderState::new(folder, Some(name.to_owned()))
        };
        self.save(&state);
        state
    }

    fn native(&self, products: Vec<folder::Product>) -> FolderState {
        self.game(
            "rimworld",
            RIMWORLD,
            Platform::Linux,
            Track::Installer,
            "en-US",
            products,
        )
    }

    fn galaxy(&self, build_id: Option<&str>) -> FolderState {
        self.game(
            "disco",
            DISCO,
            Platform::Windows,
            Track::Galaxy,
            "en-US",
            vec![built(DISCO, build_id)],
        )
    }

    fn save(&self, state: &FolderState) {
        FolderLock::exclusive(&self.paths, &state.folder, NEVER)
            .unwrap()
            .write(state)
            .unwrap();
    }

    fn state(&self, state: &FolderState) -> FolderState {
        folder::peek(&self.paths, &state.folder).unwrap().unwrap()
    }

    /// Serves GOG's token, product, and build endpoints from the maps.
    fn serve(&self) -> Scripted {
        let (products, builds) = (self.products.clone(), self.builds.clone());
        Scripted::start(move |line| {
            let path = line.split(' ').nth(1).unwrap_or_default();
            let path = path.split('?').next().unwrap_or_default();
            if path == "/token" {
                return tokens("ACCESS", "R", USER, 3600);
            }
            let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
            let id = parts.get(1).and_then(|id| id.parse::<u64>().ok());
            let found = |maps: &BTreeMap<u64, Value>| {
                id.and_then(|id| maps.get(&id))
                    .map_or_else(|| reply(404, "{}"), |body| reply(200, &body.to_string()))
            };
            match parts.as_slice() {
                ["products", _] => found(&products),
                ["products", _, "os", "windows", "builds"] => found(&builds),
                _ => reply(404, "{}"),
            }
        })
    }

    fn with<T>(&self, run: impl FnOnce(&mut Session) -> T) -> T {
        let server = self.serve();
        let mut session = Session::open(&self.paths, &*self.keyring, client_at(&server.base))
            .unwrap()
            .unwrap();
        run(&mut session)
    }

    fn check(&self, name: &str) -> Result<Status> {
        self.with(|session| check(session, name, true))
    }

    fn status(&self, name: &str) -> Status {
        self.check(name).unwrap()
    }
}

fn dlc_state(status: &Status, id: u64) -> DlcState {
    status.dlc.iter().find(|dlc| dlc.id == id).unwrap().state
}

fn cant_compare(status: &Status) -> &str {
    assert_eq!(status.state, UpdateState::CantCompare);
    assert!(!status.applicable);
    match status.reason.as_ref().unwrap() {
        Reason::CantCompare(why) => why,
        other => panic!("expected a reason to give, got {other:?}"),
    }
}

fn journal(gog_id: u64, platform: Platform, track: Track) -> Journal {
    Journal {
        operation: "op".to_owned(),
        kind: JournalKind::Update,
        target: Target {
            gog_id,
            platform,
            language: "en-US".to_owned(),
            track,
            products: Vec::new(),
        },
        packages: Vec::new(),
        attempted: Vec::new(),
    }
}

#[test]
fn a_native_game_matching_gog_is_up_to_date() {
    let mut fx = Fixture::new();
    fx.products.insert(
        RIMWORLD,
        product(
            RIMWORLD,
            "RimWorld",
            linux("1.6.4871"),
            vec![dlc(ROYALTY, "Royalty", linux("1.6.4871"))],
        ),
    );
    let state = fx.native(vec![
        installed(RIMWORLD, Some("1.6.4871")),
        installed(ROYALTY, Some("1.6.4871")),
    ]);
    let status = fx.status("rimworld");
    assert_eq!(status.state, UpdateState::UpToDate);
    assert_eq!(status.state.as_str(), "none");
    assert_eq!((status.applicable, status.reason.clone()), (false, None));
    assert_eq!(status.environment, "rimworld");
    assert_eq!(status.gog_id, RIMWORLD);
    assert_eq!(status.title, "RimWorld");
    assert_eq!(status.folder, state.folder);
    assert_eq!(status.platform, Platform::Linux);
    assert_eq!(status.track, Track::Installer);
    assert_eq!(status.installed.as_deref(), Some("1.6.4871"));
    assert_eq!(status.available.as_deref(), Some("1.6.4871"));
    assert!(!status.unfinished && status.warnings.is_empty());
    assert_eq!(
        status.dlc,
        [DlcStatus {
            id: ROYALTY,
            title: Some("Royalty".to_owned()),
            installed: Some("1.6.4871".to_owned()),
            available: Some("1.6.4871".to_owned()),
            state: DlcState::UpToDate,
        }]
    );
    // Reading changes nothing.
    assert_eq!(fx.state(&state), state);
}

#[test]
fn a_different_base_version_is_an_update_this_wineo_can_apply() {
    let mut fx = Fixture::new();
    fx.products.insert(
        RIMWORLD,
        product(RIMWORLD, "RimWorld", linux("1.6.4900"), vec![]),
    );
    fx.native(vec![installed(RIMWORLD, Some("1.6.4871"))]);
    let status = fx.status("rimworld");
    assert_eq!(status.state, UpdateState::Available);
    assert_eq!(status.state.as_str(), "available");
    assert!(status.applicable);
    assert_eq!(status.reason, None);
    assert_eq!(status.available.as_deref(), Some("1.6.4900"));
}

#[test]
fn a_dlc_only_update_is_found() {
    let mut fx = Fixture::new();
    fx.products.insert(
        RIMWORLD,
        product(
            RIMWORLD,
            "RimWorld",
            linux("1.6.4871"),
            vec![
                dlc(ROYALTY, "Royalty", linux("1.6.4871")),
                dlc(IDEOLOGY, "Ideology", linux("1.6.4900")),
            ],
        ),
    );
    fx.native(vec![
        installed(RIMWORLD, Some("1.6.4871")),
        installed(ROYALTY, Some("1.6.4871")),
        installed(IDEOLOGY, Some("1.6.4871")),
    ]);
    let status = fx.status("rimworld");
    assert_eq!(status.state, UpdateState::Available);
    assert!(status.applicable);
    assert_eq!(dlc_state(&status, ROYALTY), DlcState::UpToDate);
    assert_eq!(dlc_state(&status, IDEOLOGY), DlcState::Available);
}

#[test]
fn a_migrated_dlc_without_a_version_does_not_decide_the_game() {
    let mut fx = Fixture::new();
    let dlcs = vec![dlc(ROYALTY, "Royalty", linux("1.6.4871"))];
    fx.products.insert(
        RIMWORLD,
        product(RIMWORLD, "RimWorld", linux("1.6.4871"), dlcs.clone()),
    );
    fx.native(vec![
        installed(RIMWORLD, Some("1.6.4871")),
        installed(ROYALTY, None),
    ]);
    let status = fx.status("rimworld");
    assert_eq!(status.state, UpdateState::UpToDate);
    assert_eq!(dlc_state(&status, ROYALTY), DlcState::Unknown);
    assert_eq!(status.dlc[0].installed, None);
    assert_eq!(status.dlc[0].available.as_deref(), Some("1.6.4871"));

    // The base still decides.
    fx.products.insert(
        RIMWORLD,
        product(RIMWORLD, "RimWorld", linux("1.6.5000"), dlcs),
    );
    let status = fx.status("rimworld");
    assert_eq!(status.state, UpdateState::Available);
    assert_eq!(dlc_state(&status, ROYALTY), DlcState::Unknown);
}

#[test]
fn the_language_matches_exactly_before_its_primary_subtag() {
    let mut fx = Fixture::new();
    fx.native(vec![installed(RIMWORLD, Some("2"))]);

    // en-US is recorded, and GOG has both: the exact one is used.
    let both = vec![
        installer("linux", "en", Some("1")),
        installer("linux", "en-US", Some("2")),
    ];
    fx.products
        .insert(RIMWORLD, product(RIMWORLD, "RimWorld", both, vec![]));
    let status = fx.status("rimworld");
    assert_eq!(status.state, UpdateState::UpToDate);
    assert_eq!(status.available.as_deref(), Some("2"));

    // Only the primary subtag is offered.
    fx.products
        .insert(RIMWORLD, product(RIMWORLD, "RimWorld", linux("3"), vec![]));
    let status = fx.status("rimworld");
    assert_eq!(status.state, UpdateState::Available);
    assert_eq!(status.available.as_deref(), Some("3"));

    // Another language is no match, whatever else is offered.
    let german = vec![installer("linux", "de", Some("3"))];
    fx.products
        .insert(RIMWORLD, product(RIMWORLD, "RimWorld", german, vec![]));
    let status = fx.status("rimworld");
    assert_eq!(
        cant_compare(&status),
        "RimWorld has no Linux installer in en-US."
    );
    assert_eq!(status.available, None);

    // Two candidates are no match either.
    let twice = vec![
        installer("linux", "en", Some("3")),
        installer("linux", "en", Some("4")),
    ];
    fx.products
        .insert(RIMWORLD, product(RIMWORLD, "RimWorld", twice, vec![]));
    let status = fx.status("rimworld");
    assert!(cant_compare(&status).contains("more than one Linux installer"));

    // A Windows installer isn't a Linux one.
    let windows = vec![installer("windows", "en", Some("2"))];
    fx.products
        .insert(RIMWORLD, product(RIMWORLD, "RimWorld", windows, vec![]));
    assert_eq!(fx.status("rimworld").state, UpdateState::CantCompare);
}

#[test]
fn a_recorded_language_without_a_subtag_matches_only_itself() {
    let mut fx = Fixture::new();
    fx.game(
        "rimworld",
        RIMWORLD,
        Platform::Linux,
        Track::Installer,
        "en",
        vec![installed(RIMWORLD, Some("1"))],
    );
    let regional = vec![installer("linux", "en-US", Some("1"))];
    fx.products
        .insert(RIMWORLD, product(RIMWORLD, "RimWorld", regional, vec![]));
    assert_eq!(fx.status("rimworld").state, UpdateState::CantCompare);
}

#[test]
fn an_unknown_installed_base_version_cant_be_compared() {
    let mut fx = Fixture::new();
    fx.products.insert(
        RIMWORLD,
        product(RIMWORLD, "RimWorld", linux("1.6"), vec![]),
    );
    fx.native(vec![installed(RIMWORLD, None)]);
    let status = fx.status("rimworld");
    assert_eq!(
        cant_compare(&status),
        "The installed version of RimWorld isn't known."
    );
}

#[test]
fn gog_listing_no_version_cant_be_compared() {
    let mut fx = Fixture::new();
    let unversioned = vec![installer("linux", "en", None)];
    fx.products
        .insert(RIMWORLD, product(RIMWORLD, "RimWorld", unversioned, vec![]));
    fx.native(vec![installed(RIMWORLD, Some("1.6"))]);
    let status = fx.status("rimworld");
    assert!(cant_compare(&status).contains("lists no version"));
}

#[test]
fn an_installed_dlc_gog_no_longer_offers_makes_the_game_cant_compare() {
    let mut fx = Fixture::new();
    // Royalty's Linux installer is gone, and Ideology isn't listed at all.
    // The base game changed too, which doesn't hide either.
    let dlcs = vec![dlc(
        ROYALTY,
        "Royalty",
        vec![installer("windows", "en", Some("1"))],
    )];
    fx.products
        .insert(RIMWORLD, product(RIMWORLD, "RimWorld", linux("2"), dlcs));
    fx.native(vec![
        installed(RIMWORLD, Some("1")),
        installed(ROYALTY, Some("1")),
        installed(IDEOLOGY, None),
    ]);
    let status = fx.status("rimworld");
    assert_eq!(
        cant_compare(&status),
        "Royalty has no Linux installer in en-US."
    );
    assert_eq!(dlc_state(&status, ROYALTY), DlcState::CantCompare);
    assert_eq!(dlc_state(&status, IDEOLOGY), DlcState::CantCompare);

    // With no version recorded for it, a DLC still has to be offered.
    let state = fx.native(vec![
        installed(RIMWORLD, Some("2")),
        installed(IDEOLOGY, None),
    ]);
    let status = fx.status("rimworld");
    assert_eq!(
        cant_compare(&status),
        "DLC 1827857764 has no Linux installer in en-US."
    );
    assert_eq!(fx.state(&state), state);
}

#[test]
fn failed_and_unavailable_dlc_are_listed_and_left_out_of_the_comparison() {
    let mut fx = Fixture::new();
    fx.products
        .insert(RIMWORLD, product(RIMWORLD, "RimWorld", linux("1"), vec![]));
    let mut failed = installed(ROYALTY, Some("0"));
    failed.status = ProductStatus::Failed;
    let mut gone = installed(IDEOLOGY, None);
    gone.status = ProductStatus::Unavailable;
    fx.native(vec![installed(RIMWORLD, Some("1")), failed, gone]);
    let status = fx.status("rimworld");
    assert_eq!(status.state, UpdateState::UpToDate);
    assert_eq!(dlc_state(&status, ROYALTY), DlcState::Failed);
    assert_eq!(dlc_state(&status, IDEOLOGY), DlcState::Unavailable);
    assert_eq!(status.dlc[0].installed.as_deref(), Some("0"));
}

fn disco_on_gog(fx: &mut Fixture, items: &[(&str, Option<&str>)]) {
    fx.products
        .insert(DISCO, product(DISCO, "Disco Elysium", vec![], vec![]));
    fx.builds.insert(DISCO, builds(items));
}

#[test]
fn a_galaxy_game_is_compared_by_build() {
    let mut fx = Fixture::new();
    // Newest first, as GOG lists them: the first with no branch is the
    // latest on the default branch.
    disco_on_gog(
        &mut fx,
        &[
            ("500", Some("beta")),
            ("400", None),
            ("300", None),
            ("200", Some("beta")),
        ],
    );

    fx.galaxy(Some("400"));
    let status = fx.status("disco");
    assert_eq!(status.state, UpdateState::UpToDate);
    assert_eq!(status.installed.as_deref(), Some("400"));
    assert_eq!(status.available.as_deref(), Some("400"));
    assert_eq!((status.applicable, status.reason.clone()), (false, None));
    assert_eq!(status.platform, Platform::Windows);
    assert_eq!(status.track, Track::Galaxy);

    fx.galaxy(Some("300"));
    let status = fx.status("disco");
    assert_eq!(status.state, UpdateState::OnGog);
    assert_eq!(status.state.as_str(), "galaxy");
    assert!(!status.applicable);
    assert_eq!(status.reason, Some(Reason::Galaxy));
    assert_eq!(status.reason.as_ref().unwrap().as_str(), "galaxy");
    assert_eq!(status.available.as_deref(), Some("400"));

    fx.galaxy(Some("200"));
    assert!(cant_compare(&fx.status("disco")).contains("beta branch"));
    fx.galaxy(Some("999"));
    assert!(cant_compare(&fx.status("disco")).contains("isn't one GOG lists"));
    fx.galaxy(None);
    assert!(cant_compare(&fx.status("disco")).contains("isn't known"));

    fx.builds.insert(DISCO, builds(&[("500", Some("beta"))]));
    fx.galaxy(Some("500"));
    assert!(cant_compare(&fx.status("disco")).contains("default branch"));
}

#[test]
fn a_galaxy_check_fails_when_the_build_list_cant_be_had() {
    let mut fx = Fixture::new();
    fx.products
        .insert(DISCO, product(DISCO, "Disco Elysium", vec![], vec![]));
    fx.galaxy(Some("400"));
    // The server has no build list, so it answers 404.
    let error = fx.check("disco").unwrap_err();
    assert!(error.downcast_ref::<account::CallError>().is_some());
}

/// Writes the file another tool's update leaves in a Windows game folder.
fn info(folder: &Path, game_id: &str, build: &str) {
    fs::write(
        folder.join(format!("goggame-{DISCO}.info")),
        json!({"gameId": game_id, "buildId": build}).to_string(),
    )
    .unwrap();
}

#[test]
fn a_build_another_tool_installed_is_recorded_and_used() {
    let mut fx = Fixture::new();
    disco_on_gog(&mut fx, &[("400", None), ("300", None)]);
    let state = fx.galaxy(Some("300"));
    info(&state.folder, &DISCO.to_string(), "400");
    let status = fx.status("disco");
    assert_eq!(status.state, UpdateState::UpToDate);
    assert_eq!(status.installed.as_deref(), Some("400"));
    assert_eq!(
        status.warnings,
        ["Recorded the build 400, which another tool installed."]
    );
    let after = fx.state(&state);
    let committed = after.committed.as_ref().unwrap();
    assert_eq!(committed.products[0].build.as_deref(), Some("400"));
    assert_eq!(after.owner, state.owner);
    assert!(after.journal.is_none());
    assert!(after.device.is_some());

    // The next check has nothing to record.
    assert!(fx.status("disco").warnings.is_empty());

    // An older build on disk is recorded too, and compared as it is.
    info(&state.folder, &DISCO.to_string(), "300");
    assert_eq!(fx.status("disco").state, UpdateState::OnGog);
    assert_eq!(
        fx.state(&state).committed.unwrap().products[0]
            .build
            .as_deref(),
        Some("300")
    );
}

#[test]
fn a_build_on_disk_is_not_trusted_without_the_games_own_id() {
    let mut fx = Fixture::new();
    disco_on_gog(&mut fx, &[("400", None), ("300", None)]);
    let state = fx.galaxy(Some("300"));
    for game_id in ["1", ""] {
        info(&state.folder, game_id, "400");
        let status = fx.status("disco");
        assert_eq!(status.state, UpdateState::OnGog);
        assert_eq!(status.installed.as_deref(), Some("300"));
        assert_eq!(fx.state(&state), state);
    }
    // An empty build says nothing, and neither does a file that isn't one.
    info(&state.folder, &DISCO.to_string(), "");
    assert_eq!(fx.status("disco").installed.as_deref(), Some("300"));
    fs::write(state.folder.join(format!("goggame-{DISCO}.info")), "{").unwrap();
    assert_eq!(fx.status("disco").installed.as_deref(), Some("300"));
    assert_eq!(fx.state(&state), state);
}

#[test]
fn a_build_on_disk_is_not_recorded_while_a_journal_is_present() {
    let mut fx = Fixture::new();
    disco_on_gog(&mut fx, &[("400", None), ("300", None)]);
    let mut state = fx.galaxy(Some("300"));
    state.journal = Some(journal(DISCO, Platform::Windows, Track::Galaxy));
    fx.save(&state);
    info(&state.folder, &DISCO.to_string(), "400");
    let status = fx.status("disco");
    assert!(status.unfinished);
    // Computed from the committed build.
    assert_eq!(status.state, UpdateState::OnGog);
    assert_eq!(status.installed.as_deref(), Some("300"));
    assert_eq!(fx.state(&state), state);
}

#[test]
fn a_state_that_changed_since_it_was_read_is_not_overwritten() {
    let mut fx = Fixture::new();
    disco_on_gog(&mut fx, &[("400", None), ("300", None)]);
    let state = fx.galaxy(Some("300"));
    info(&state.folder, &DISCO.to_string(), "400");

    // A journal appeared after the state was read.
    let mut journaled = state.clone();
    journaled.journal = Some(journal(DISCO, Platform::Windows, Track::Galaxy));
    fx.save(&journaled);
    assert_eq!(adopt_build(&fx.paths, &state).unwrap(), None);
    assert_eq!(fx.state(&state), journaled);

    // The owner changed.
    let mut released = state.clone();
    released.owner = Some("other".to_owned());
    fx.save(&released);
    assert_eq!(adopt_build(&fx.paths, &state).unwrap(), None);
    assert_eq!(fx.state(&state), released);

    // The track changed.
    let mut tracked = state.clone();
    tracked.committed.as_mut().unwrap().track = Track::Installer;
    fx.save(&tracked);
    assert_eq!(adopt_build(&fx.paths, &state).unwrap(), None);
    assert_eq!(fx.state(&state), tracked);

    // Unchanged, it is recorded.
    fx.save(&state);
    assert!(adopt_build(&fx.paths, &state).unwrap().is_some());
}

#[test]
fn a_failed_record_of_another_tools_build_fails_that_game() {
    let mut fx = Fixture::new();
    disco_on_gog(&mut fx, &[("400", None), ("300", None)]);
    let state = fx.galaxy(Some("300"));
    info(&state.folder, &DISCO.to_string(), "400");
    set_failpoint(Some("write.before"));
    let error = fx.check("disco").unwrap_err();
    set_failpoint(None);
    assert!(error.to_string().contains("injected"), "{error:#}");
    assert_eq!(fx.state(&state), state);
}

#[test]
fn only_a_galaxy_game_trusts_its_info_file() {
    let mut fx = Fixture::new();
    fx.products.insert(
        DISCO,
        product(
            DISCO,
            "Disco Elysium",
            vec![installer("windows", "en", Some("1.0"))],
            vec![],
        ),
    );
    let state = fx.game(
        "disco",
        DISCO,
        Platform::Windows,
        Track::Installer,
        "en-US",
        vec![installed(DISCO, Some("1.0"))],
    );
    info(&state.folder, &DISCO.to_string(), "400");
    assert_eq!(fx.status("disco").state, UpdateState::UpToDate);
    assert_eq!(fx.state(&state), state);
}

#[test]
fn a_windows_installer_game_shows_an_update_it_cant_apply_yet() {
    let mut fx = Fixture::new();
    fx.products.insert(
        DISCO,
        product(
            DISCO,
            "Disco Elysium",
            vec![installer("windows", "en", Some("2.0"))],
            vec![],
        ),
    );
    fx.game(
        "disco",
        DISCO,
        Platform::Windows,
        Track::Installer,
        "en-US",
        vec![installed(DISCO, Some("1.0"))],
    );
    let status = fx.status("disco");
    assert_eq!(status.state, UpdateState::Available);
    assert!(!status.applicable);
    assert_eq!(status.reason, Some(Reason::WindowsInstaller));
    assert_eq!(
        status.reason.as_ref().unwrap().as_str(),
        "windows_installer"
    );
    assert_eq!(status.installed.as_deref(), Some("1.0"));
    assert_eq!(status.available.as_deref(), Some("2.0"));
}

#[test]
fn a_journal_sets_unfinished_and_status_comes_from_the_committed_state() {
    let mut fx = Fixture::new();
    fx.products
        .insert(RIMWORLD, product(RIMWORLD, "RimWorld", linux("2"), vec![]));
    let mut state = fx.native(vec![installed(RIMWORLD, Some("1"))]);
    let mut pending = journal(RIMWORLD, Platform::Linux, Track::Installer);
    // The journal targets 2, but what is installed is still 1.
    pending.target.products = vec![installed(RIMWORLD, Some("2"))];
    state.journal = Some(pending);
    fx.save(&state);
    let status = fx.status("rimworld");
    assert!(status.unfinished);
    assert_eq!(status.state, UpdateState::Available);
    assert_eq!(status.installed.as_deref(), Some("1"));
}

#[test]
fn a_first_install_still_in_flight_is_unfinished_and_cant_be_compared() {
    let mut fx = Fixture::new();
    fx.products
        .insert(RIMWORLD, product(RIMWORLD, "RimWorld", linux("2"), vec![]));
    let mut state = fx.native(Vec::new());
    state.committed = None;
    state.journal = Some(journal(RIMWORLD, Platform::Linux, Track::Installer));
    fx.save(&state);
    let status = fx.status("rimworld");
    assert!(status.unfinished);
    assert_eq!(cant_compare(&status), "The install hasn't finished.");
    assert!(status.dlc.is_empty());
}

#[test]
fn a_lost_state_is_an_error() {
    let fx = Fixture::new();
    let state = fx.native(vec![installed(RIMWORLD, Some("1"))]);
    fs::remove_file(folder::state_path(&fx.paths, &state.folder)).unwrap();
    let error = fx.check("rimworld").unwrap_err();
    assert_eq!(
        error.to_string(),
        "rimworld's GOG state is missing. Migrate it again with wineo import gog --game 1094900565, or stop tracking it with wineo set rimworld --unset gog_id."
    );
}

#[test]
fn an_unreadable_state_names_its_file() {
    let fx = Fixture::new();
    let state = fx.native(vec![installed(RIMWORLD, Some("1"))]);
    let file = folder::state_path(&fx.paths, &state.folder);
    fs::write(&file, "not [ toml").unwrap();
    let message = fx.check("rimworld").unwrap_err().to_string();
    assert!(
        message.starts_with("rimworld's GOG state can't be checked: "),
        "{message}"
    );
    assert!(message.contains(&file.display().to_string()), "{message}");
}

#[test]
fn an_environment_that_tracks_no_game_is_refused() {
    let fx = Fixture::new();
    let path = fx.paths.environment_config("plain");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "kind = \"native\"\n\n[targets.game]\nexe = \"/x\"\n").unwrap();
    assert_eq!(
        fx.check("plain").unwrap_err().to_string(),
        "plain doesn't track a GOG game."
    );
    assert!(fx.check("missing").is_err());
}

#[test]
fn details_that_cant_be_fetched_fail_the_game() {
    let fx = Fixture::new();
    fx.native(vec![installed(RIMWORLD, Some("1"))]);
    let error = fx.check("rimworld").unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("Couldn't get GOG product details: "),
        "{error:#}"
    );
}

#[test]
fn old_details_stand_in_with_their_warning() {
    let mut fx = Fixture::new();
    fx.products
        .insert(RIMWORLD, product(RIMWORLD, "RimWorld", linux("1"), vec![]));
    fx.native(vec![installed(RIMWORLD, Some("1"))]);
    assert!(fx.status("rimworld").warnings.is_empty());
    fx.products.clear();
    let status = fx.status("rimworld");
    assert_eq!(status.state, UpdateState::UpToDate);
    assert_eq!(status.warnings.len(), 1);
    assert!(
        status.warnings[0].starts_with("Couldn't refresh GOG product details: "),
        "{:?}",
        status.warnings
    );
}

#[test]
fn a_batch_checks_every_owned_game_and_keeps_going_past_a_bad_one() {
    let mut fx = Fixture::new();
    fx.products
        .insert(RIMWORLD, product(RIMWORLD, "RimWorld", linux("2"), vec![]));
    fx.products
        .insert(DISCO, product(DISCO, "Disco Elysium", vec![], vec![]));
    fx.builds.insert(DISCO, builds(&[("400", None)]));
    let rimworld = fx.native(vec![installed(RIMWORLD, Some("1"))]);
    let disco = fx.galaxy(Some("400"));

    let one = |name: &str, id: u64| {
        fx.game(
            name,
            id,
            Platform::Linux,
            Track::Installer,
            "en",
            vec![installed(id, Some("1"))],
        )
    };
    // A game whose details can't be fetched.
    one("aaa-broken", 42);
    // An ownerless folder, one whose owner tracks another game, and one
    // whose owner is gone are skipped.
    let ownerless = one("ownerless", 7);
    FolderLock::exclusive(&fx.paths, &ownerless.folder, NEVER)
        .unwrap()
        .release()
        .unwrap();
    one("elsewhere", 8);
    fs::write(
        fx.paths.environment_config("elsewhere"),
        "kind = \"native\"\ngog_id = \"9\"\n",
    )
    .unwrap();
    one("orphan", 10);
    fs::remove_file(fx.paths.environment_config("orphan")).unwrap();
    // A state file that can't be read says so, without an owner.
    let hopeless = fs::canonicalize(fx.root.path())
        .unwrap()
        .join("Games/hopeless");
    fs::create_dir_all(&hopeless).unwrap();
    let file = folder::state_path(&fx.paths, &hopeless);
    fs::write(&file, "nonsense [").unwrap();

    let batch = fx.with(|session| check_all(session, true)).unwrap();
    let names: Vec<_> = batch
        .statuses
        .iter()
        .map(|status| status.environment.as_str())
        .collect();
    assert_eq!(names, ["disco", "rimworld"]);
    assert_eq!(batch.statuses[0].state, UpdateState::UpToDate);
    assert_eq!(batch.statuses[1].state, UpdateState::Available);

    assert_eq!(batch.errors.len(), 2, "{:?}", batch.errors);
    assert_eq!(batch.errors[0].environment, None);
    assert_eq!(batch.errors[0].id, None);
    assert!(
        batch.errors[0]
            .message
            .contains(&file.display().to_string()),
        "{:?}",
        batch.errors[0]
    );
    assert_eq!(batch.errors[1].environment.as_deref(), Some("aaa-broken"));
    assert_eq!(batch.errors[1].id, Some(42));
    assert!(
        batch.errors[1]
            .message
            .starts_with("Couldn't get GOG product details: ")
    );
    assert_eq!(fx.state(&rimworld), rimworld);
    assert_eq!(fx.state(&disco), disco);
}

#[test]
fn a_portable_root_is_refused() {
    let fx = Fixture::new();
    let home = fx.root.path().to_owned();
    let portable = Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())),
        Some(&fx.root.path().join("usb")),
    )
    .unwrap();
    let server = fx.serve();
    let mut session = Session::open(&portable, &*fx.keyring, client_at(&server.base))
        .unwrap()
        .unwrap();
    for error in [
        check(&mut session, "rimworld", true).unwrap_err(),
        check_all(&mut session, true).unwrap_err(),
    ] {
        assert!(matches!(
            error.downcast_ref::<MigrateError>(),
            Some(MigrateError::Portable)
        ));
        assert_eq!(
            error.to_string(),
            "GOG games live in the host library. Run this without --portable."
        );
    }
    assert!(server.requests().is_empty());
}
