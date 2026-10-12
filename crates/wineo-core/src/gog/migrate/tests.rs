use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Barrier};

use tempfile::TempDir;

use super::*;
use crate::config::Defaults;
use crate::gog::folder::{
    FolderError, InUseLock, Journal, JournalKind, Target as JournalTarget, set_failpoint,
};
use crate::test_snapshot::{SnapshotEntry, assert_unchanged, snapshot_tree};

type Hook = Box<dyn FnOnce()>;

thread_local! {
    static HOOKS: std::cell::RefCell<Vec<(&'static str, Hook)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Runs the hook set on this thread for `at`, once.
fn run_hook(at: &str) {
    let hook = HOOKS.with(|hooks| {
        let mut hooks = hooks.borrow_mut();
        let index = hooks.iter().position(|(point, _)| *point == at)?;
        Some(hooks.remove(index).1)
    });
    if let Some(hook) = hook {
        hook();
    }
}

fn on(at: &'static str, hook: impl FnOnce() + 'static) {
    HOOKS.with(|hooks| hooks.borrow_mut().push((at, Box::new(hook))));
}

/// Between creating a new environment and the locked pass that writes its
/// state.
pub(super) fn after_create() {
    run_hook("after_create");
}

/// Under the folder's locks, just before `gog_id` is recorded.
pub(super) fn before_record() {
    run_hook("before_record");
}

/// In planning, after the game's files were read and before they are
/// checked again.
pub(super) fn after_reading() {
    run_hook("after_reading");
}

fn on_create(hook: impl FnOnce() + 'static) {
    on("after_create", hook);
}

const RIMWORLD: &str = "1094900565";
const DISCO: &str = "1771589310";
const RIMWORLD_DLC: [u64; 3] = [1233017772, 1827857764, 1865538500];

/// A home with a Heroic config, a games folder, and a fake wine runner.
struct Fixture {
    root: TempDir,
    paths: Paths,
    global: GlobalConfig,
    source: HeroicSource,
    installed: Vec<serde_json::Value>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_owned();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
                .unwrap();
        let heroic = root.path().join("heroic");
        fs::create_dir_all(heroic.join("GamesConfig")).unwrap();
        fs::create_dir_all(heroic.join("gog_store")).unwrap();
        fs::create_dir_all(heroic.join("store_cache")).unwrap();
        fs::write(
            heroic.join("config.json"),
            r#"{"defaultSettings":{"wineVersion":{"type":"wine","name":"System","bin":"wine"}}}"#,
        )
        .unwrap();
        fs::write(
            heroic.join("store_cache/gog_library.json"),
            format!(
                r#"{{"games":[{{"runner":"gog","app_name":"{RIMWORLD}","title":"RimWorld","is_linux_native":true}},{{"runner":"gog","app_name":"{DISCO}","title":"Disco Elysium"}}],"__timestamp":1}}"#
            ),
        )
        .unwrap();
        let runner = root.path().join("runners/fake/bin");
        fs::create_dir_all(&runner).unwrap();
        for executable in ["wine", "wineserver"] {
            let path = runner.join(executable);
            fs::write(&path, "#!/bin/sh\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let global = GlobalConfig {
            defaults: Defaults {
                runner_root: Some(root.path().join("runners").display().to_string()),
                prefix_root: Some(root.path().join("prefixes").display().to_string()),
                ..Defaults::default()
            },
            ..GlobalConfig::default()
        };
        let source = HeroicSource::from_config_dir_with_home(heroic, root.path().to_owned());
        let fixture = Self {
            root,
            paths,
            global,
            source,
            installed: Vec::new(),
        };
        fixture.write_installed();
        fixture
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    fn heroic(&self) -> PathBuf {
        self.source.config_dir().to_owned()
    }

    fn write_installed(&self) {
        fs::write(
            self.heroic().join("gog_store/installed.json"),
            serde_json::json!({ "installed": self.installed }).to_string(),
        )
        .unwrap();
    }

    fn install(&mut self, entry: serde_json::Value) {
        self.installed.push(entry);
        self.write_installed();
    }

    /// RimWorld, native, in `<folder>`, with its three DLC and a mod folder
    /// it doesn't ship.
    fn rimworld_in(&mut self, folder: &str) -> PathBuf {
        let dir = self.path(folder);
        fs::create_dir_all(dir.join("mods-1.5/MyMod")).unwrap();
        fs::write(dir.join("mods-1.5/MyMod/About.xml"), "mine").unwrap();
        fs::write(
            dir.join("gameinfo"),
            format!(
                "RimWorld\n1.6.4871 rev588\n93132\nen-US\n{RIMWORLD}\n{RIMWORLD}\n59819264299049740\n"
            ),
        )
        .unwrap();
        let start = dir.join("start.sh");
        fs::write(&start, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&start, fs::Permissions::from_mode(0o755)).unwrap();
        self.install(serde_json::json!({
            "platform": "linux", "executable": "", "install_path": dir,
            "install_size": "5.03 GiB", "is_dlc": false, "version": "1.6.4871 rev588",
            "appName": RIMWORLD, "installedDLCs": RIMWORLD_DLC,
            "language": "en", "versionEtag": "", "buildId": "", "pinnedVersion": false,
        }));
        dir
    }

    fn rimworld(&mut self) -> PathBuf {
        self.rimworld_in("Games/RimWorld")
    }

    /// Disco Elysium, a Windows game in `<folder>` with its own wine prefix,
    /// installed from Galaxy unless `build` is empty.
    fn disco_in(&mut self, folder: &str, build: &str) -> PathBuf {
        let dir = self.path(folder);
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(dir.join("Disco Elysium.exe"), "game").unwrap();
        fs::write(
            dir.join(format!("goggame-{DISCO}.info")),
            serde_json::json!({
                "gameId": DISCO,
                "rootGameId": DISCO,
                "buildId": build,
                "playTasks": [
                    {"category": "document", "type": "FileTask", "path": "manual.pdf"},
                    {
                        "isPrimary": true,
                        "type": "FileTask",
                        "path": "Disco Elysium.exe",
                        "workingDir": "bin",
                        "arguments": "-a \"b c\"",
                        "category": "game",
                    },
                ],
            })
            .to_string(),
        )
        .unwrap();
        let prefix = self.path("Prefixes/Disco");
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        fs::write(
            prefix.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        fs::write(
            self.heroic().join(format!("GamesConfig/{DISCO}.json")),
            serde_json::json!({
                DISCO: {
                    "winePrefix": prefix,
                    "wineVersion": {"type": "wine", "name": "System", "bin": "wine"},
                    "launcherArgs": "--skip-intro",
                    "enviromentOptions": [{"key": "FOO", "value": "1"}],
                    "useGameMode": true,
                }
            })
            .to_string(),
        )
        .unwrap();
        self.install(serde_json::json!({
            "platform": "windows", "executable": "", "install_path": dir,
            "install_size": "9.57 GiB", "is_dlc": false, "version": "WINkay16062026_memfix",
            "appName": DISCO, "installedDLCs": [], "language": "en-US",
            "versionEtag": "\"0576\"", "buildId": build, "pinnedVersion": false,
        }));
        dir
    }

    fn disco(&mut self) -> PathBuf {
        self.disco_in("Games/Disco Elysium - The Final Cut", "59781086834418085")
    }

    fn environment(&self, name: &str, text: &str) {
        let path = self.paths.environment_config(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// A native environment that runs `exe` by default.
    fn native(&self, name: &str, exe: &Path, extra: &str) -> String {
        let text = format!(
            "schema_version = 1\n# mine\nkind = \"native\"\ntags = [\"game\"]\n{extra}\n[targets.game]\nexe = \"{}\"\ndefault = true\n",
            exe.display()
        );
        self.environment(name, &text);
        text
    }

    fn config(&self, name: &str) -> String {
        fs::read_to_string(self.paths.environment_config(name)).unwrap()
    }

    fn plan(&self, id: &str) -> Result<Plan> {
        plan(&self.paths, &self.source, id)
    }

    fn migration(&self, id: &str) -> Migration {
        match self.plan(id).unwrap() {
            Plan::Migrate(migration) => migration,
            other => panic!("expected a migration, got {other:?}"),
        }
    }

    fn apply(&self, migration: &Migration) -> Result<Outcome> {
        apply(
            &self.paths,
            &self.global,
            &self.source,
            migration,
            &Token::new(),
            |_| {},
        )
    }

    fn migrated(&self, migration: &Migration) -> Migrated {
        match self.apply(migration).unwrap() {
            Outcome::Migrated(migrated) => migrated,
            other => panic!("expected a migration, got {other:?}"),
        }
    }

    fn state(&self, folder: &Path) -> Option<FolderState> {
        folder::peek(&self.paths, &folder::canonical(folder).unwrap()).unwrap()
    }

    /// Everything under Heroic's config, with contents.
    fn heroic_files(&self) -> Frozen {
        Frozen::of(&self.heroic())
    }

    fn write_state(&self, state: &FolderState) {
        FolderLock::exclusive(&self.paths, &state.folder, &|| false)
            .unwrap()
            .write(state)
            .unwrap();
    }

    fn rimworld_committed(&self, gog_id: u64) -> Committed {
        committed(&self.source.gog_game(RIMWORLD).unwrap().unwrap(), gog_id)
    }
}

/// A tree's entries and file contents, to show nothing changed.
struct Frozen {
    root: PathBuf,
    tree: Vec<SnapshotEntry>,
    bytes: BTreeMap<PathBuf, Vec<u8>>,
}

impl Frozen {
    fn of(root: &Path) -> Self {
        let mut bytes = BTreeMap::new();
        let mut stack = vec![root.to_owned()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    bytes.insert(path.clone(), fs::read(&path).unwrap());
                }
            }
        }
        Self {
            root: root.to_owned(),
            tree: snapshot_tree(root),
            bytes,
        }
    }

    fn assert_unchanged(&self) {
        let now = Self::of(&self.root);
        assert_unchanged(&self.tree, &now.tree);
        assert_eq!(self.bytes, now.bytes);
    }
}

fn products(ids: &[(u64, Option<&str>, Option<&str>)]) -> Vec<Product> {
    ids.iter()
        .map(|(id, version, build)| Product {
            id: *id,
            version: version.map(str::to_owned),
            build: build.map(str::to_owned),
            status: ProductStatus::Installed,
        })
        .collect()
}

fn migrate_error(result: Result<impl std::fmt::Debug>) -> MigrateError {
    result
        .unwrap_err()
        .downcast::<MigrateError>()
        .unwrap_or_else(|error| panic!("{error:#}"))
}

fn folder_error(result: Result<impl std::fmt::Debug>) -> FolderError {
    result
        .unwrap_err()
        .downcast::<FolderError>()
        .unwrap_or_else(|error| panic!("{error:#}"))
}

#[test]
fn native_rimworld_attaches_to_the_environment_running_start_sh() {
    let mut fx = Fixture::new();
    let folder = fx.rimworld();
    let before = fx.native("rimworld", &folder.join("start.sh"), "");
    let heroic = fx.heroic_files();
    let game = Frozen::of(&folder);

    let migration = fx.migration(RIMWORLD);
    assert!(matches!(
        migration.target(),
        Target::Existing { name, records_gog_id: true } if name == "rimworld"
    ));
    assert_eq!(migration.state(), StateChange::Adopt);
    // Planning writes nothing.
    assert_eq!(fx.config("rimworld"), before);
    assert!(fx.state(&folder).is_none());

    let migrated = fx.migrated(&migration);
    assert_eq!(migrated.environment, "rimworld");
    assert_eq!(migrated.state, StateChange::Adopt);
    assert!(migrated.recorded_gog_id);
    assert!(migrated.registered.is_none());

    // Only gog_id is added to the config, comments and all kept.
    let after = fx.config("rimworld");
    let line = format!("gog_id = \"{RIMWORLD}\"");
    let added: Vec<&str> = after
        .lines()
        .filter(|new| !before.lines().any(|old| old == *new))
        .collect();
    assert_eq!(added, [line.as_str()]);
    assert_eq!(after.replace(&format!("{line}\n"), ""), before);

    let state = fx.state(&folder).unwrap();
    assert_eq!(state.owner.as_deref(), Some("rimworld"));
    assert_eq!(state.folder, folder::canonical(&folder).unwrap());
    assert_eq!(state.device, Some(fs::metadata(&folder).unwrap().dev()));
    assert!(state.journal.is_none());
    let committed = state.committed.unwrap();
    assert_eq!(committed.gog_id, 1094900565);
    assert_eq!(committed.platform, Platform::Linux);
    assert_eq!(committed.language, "en");
    assert_eq!(committed.track, Track::Installer);
    assert_eq!(
        committed.products,
        products(&[
            (1094900565, Some("1.6.4871 rev588"), None),
            (RIMWORLD_DLC[0], None, None),
            (RIMWORLD_DLC[1], None, None),
            (RIMWORLD_DLC[2], None, None),
        ])
    );
    assert!(committed.files.is_empty());

    heroic.assert_unchanged();
    game.assert_unchanged();
}

#[test]
fn a_galaxy_windows_game_becomes_a_new_environment_from_heroic_settings() {
    let mut fx = Fixture::new();
    let folder = fx.disco();
    let heroic = fx.heroic_files();

    let mut migration = fx.migration(DISCO);
    assert!(matches!(
        migration.target(),
        Target::Windows { plan: None, .. }
    ));
    assert_eq!(migration.environment(), None);
    assert!(matches!(
        migrate_error(fx.apply(&migration)),
        MigrateError::NeedsRunner
    ));
    migration
        .plan_runner(&fx.paths, &fx.global, "fake", None, None)
        .unwrap();
    let name = migration.environment().unwrap().to_owned();
    assert!(!migration.name_taken(&fx.paths));

    let migrated = fx.migrated(&migration);
    assert_eq!(migrated.environment, name);
    assert_eq!(migrated.state, StateChange::Adopt);
    let registered = migrated.registered.unwrap();
    let config = &registered.environment.config;
    assert_eq!(config.gog_id.as_deref(), Some(DISCO));
    assert_eq!(config.heroic_id.as_deref(), Some(DISCO));
    assert_eq!(config.gamemode, Some(true));
    assert_eq!(config.env.get("FOO").map(String::as_str), Some("1"));
    // Heroic's prefix is adopted where it is.
    assert_eq!(
        registered.environment.prefix(&fx.paths).unwrap(),
        fx.path("Prefixes/Disco")
    );
    let (_, target) = launch::resolve_target(&registered.environment, None).unwrap();
    assert_eq!(
        Path::new(&target.exe),
        folder.join("Disco Elysium.exe").as_path()
    );
    assert_eq!(
        target.cwd.as_deref().map(Path::new),
        Some(folder.join("bin").as_path())
    );
    assert_eq!(target.args, ["-a", "b c", "--skip-intro"]);

    let state = fx.state(&folder).unwrap();
    assert_eq!(state.owner.as_deref(), Some(name.as_str()));
    let committed = state.committed.unwrap();
    assert_eq!(committed.platform, Platform::Windows);
    assert_eq!(committed.language, "en-US");
    assert_eq!(committed.track, Track::Galaxy);
    assert_eq!(
        committed.products,
        products(&[(1771589310, None, Some("59781086834418085"))])
    );
    heroic.assert_unchanged();
}

#[test]
fn a_windows_game_without_a_build_is_on_the_installer_track() {
    let mut fx = Fixture::new();
    let folder = fx.disco_in("Games/Disco", "");
    let heroic = fx.heroic_files();
    let mut migration = fx.migration(DISCO);
    assert_eq!(migration.committed().track, Track::Installer);
    migration
        .plan_runner(
            &fx.paths,
            &fx.global,
            "fake",
            Some("disco".to_owned()),
            None,
        )
        .unwrap();
    fx.migrated(&migration);
    let committed = fx.state(&folder).unwrap().committed.unwrap();
    assert_eq!(committed.track, Track::Installer);
    assert_eq!(
        committed.products,
        products(&[(1771589310, Some("WINkay16062026_memfix"), None)])
    );
    heroic.assert_unchanged();
}

#[test]
fn a_matching_windows_environment_uses_heroic_s_prefix() {
    let mut fx = Fixture::new();
    let folder = fx.disco();
    let target = format!(
        "[targets.disco]\nexe = \"{}\"\ndefault = true\n",
        folder.join("Disco Elysium.exe").display()
    );
    // The same exe in another prefix is another environment.
    let other = fx.path("prefixes/other");
    fs::create_dir_all(other.join("drive_c")).unwrap();
    fx.environment(
        "elsewhere",
        &format!(
            "schema_version = 1\nrunner = \"fake\"\narch = \"win64\"\nprefix = \"{}\"\n\n{target}",
            other.display()
        ),
    );
    fx.environment(
        "disco",
        &format!(
            "schema_version = 1\nrunner = \"fake\"\narch = \"win64\"\nprefix = \"{}\"\n\n{target}",
            fx.path("Prefixes/Disco").display()
        ),
    );
    let heroic = fx.heroic_files();
    let migration = fx.migration(DISCO);
    assert!(matches!(
        migration.target(),
        Target::Existing { name, .. } if name == "disco"
    ));
    fx.migrated(&migration);
    assert!(
        fx.config("disco")
            .contains(&format!("gog_id = \"{DISCO}\""))
    );
    assert!(!fx.config("elsewhere").contains("gog_id"));
    heroic.assert_unchanged();
}

#[test]
fn gog_id_matches_before_paths_and_finishes_a_lost_state() {
    let mut fx = Fixture::new();
    let folder = fx.rimworld();
    // Has the game's id but runs something else, and its state is lost.
    let tracked = fx.native(
        "tracked",
        &fx.path("elsewhere/run.sh"),
        &format!("gog_id = \"{RIMWORLD}\"\n"),
    );
    fx.native("by-path", &folder.join("start.sh"), "");
    let heroic = fx.heroic_files();

    let migration = fx.migration(RIMWORLD);
    assert!(matches!(
        migration.target(),
        Target::Existing { name, records_gog_id: false } if name == "tracked"
    ));
    let migrated = fx.migrated(&migration);
    assert!(!migrated.recorded_gog_id);
    assert_eq!(fx.config("tracked"), tracked);
    assert_eq!(fx.state(&folder).unwrap().owner.as_deref(), Some("tracked"));
    assert!(!fx.config("by-path").contains("gog_id"));
    heroic.assert_unchanged();
}

#[test]
fn more_than_one_match_or_another_game_is_refused_with_names() {
    let mut fx = Fixture::new();
    let folder = fx.rimworld();
    let start = folder.join("start.sh");
    fx.native("one", &start, "");
    fx.native("two", &start, "");
    let heroic = fx.heroic_files();
    assert_eq!(
        fx.plan(RIMWORLD).unwrap_err().to_string(),
        "GOG game 1094900565 matches more than one environment: one, two."
    );

    fs::remove_file(fx.paths.environment_config("two")).unwrap();
    fx.native("one", &start, "gog_id = \"42\"\n");
    assert!(matches!(
        migrate_error(fx.plan(RIMWORLD)),
        MigrateError::OtherGame { name, other, .. } if name == "one" && other == "42"
    ));

    fx.native("one", &start, &format!("gog_id = \"{RIMWORLD}\"\n"));
    fx.native(
        "two",
        &fx.path("x/run.sh"),
        &format!("gog_id = \"{RIMWORLD}\"\n"),
    );
    assert!(matches!(
        migrate_error(fx.plan(RIMWORLD)),
        MigrateError::Ambiguous { names, .. } if names == ["one", "two"]
    ));
    assert!(fx.state(&folder).is_none());
    heroic.assert_unchanged();
}

#[test]
fn the_folder_has_to_hold_heroic_s_game() {
    let mut fx = Fixture::new();
    let rimworld = fx.rimworld();
    let disco = fx.disco();
    let heroic = fx.heroic_files();

    // gameinfo's fifth line names another product.
    let gameinfo = rimworld.join("gameinfo");
    let text = fs::read_to_string(&gameinfo).unwrap();
    fs::write(
        &gameinfo,
        text.replace(&format!("\n{RIMWORLD}\n{RIMWORLD}\n"), "\n7\n7\n"),
    )
    .unwrap();
    assert!(matches!(
        migrate_error(fx.plan(RIMWORLD)),
        MigrateError::IdMismatch { found, .. } if found == "7"
    ));
    fs::remove_file(&gameinfo).unwrap();
    assert!(matches!(
        migrate_error(fx.plan(RIMWORLD)),
        MigrateError::MissingInfo { file, .. } if file == "gameinfo"
    ));

    // goggame-<id>.info names another game.
    let info = disco.join(format!("goggame-{DISCO}.info"));
    let text = fs::read_to_string(&info).unwrap();
    fs::write(
        &info,
        text.replace(&format!("\"gameId\":\"{DISCO}\""), "\"gameId\":\"8\""),
    )
    .unwrap();
    assert!(matches!(
        migrate_error(fx.plan(DISCO)),
        MigrateError::IdMismatch { found, .. } if found == "8"
    ));
    // Only another game's info file is there.
    fs::rename(&info, disco.join("goggame-8.info")).unwrap();
    assert!(matches!(
        migrate_error(fx.plan(DISCO)),
        MigrateError::IdMismatch { found, .. } if found == "8"
    ));
    fs::remove_file(disco.join("goggame-8.info")).unwrap();
    assert!(matches!(
        migrate_error(fx.plan(DISCO)),
        MigrateError::MissingInfo { file, .. } if file == format!("goggame-{DISCO}.info")
    ));

    fs::remove_dir_all(&disco).unwrap();
    assert!(matches!(
        migrate_error(fx.plan(DISCO)),
        MigrateError::MissingFolder(missing) if missing == disco
    ));
    assert!(!fx.paths.environments_dir().exists());
    assert!(!fx.paths.gog_folders_dir().exists());
    heroic.assert_unchanged();
}

#[test]
fn play_tasks_have_to_stay_inside_the_folder() {
    let mut fx = Fixture::new();
    let disco = fx.disco();
    let heroic = fx.heroic_files();
    let info = disco.join(format!("goggame-{DISCO}.info"));
    let text = fs::read_to_string(&info).unwrap();
    fs::write(&info, text.replace("\"bin\"", "\"..\\\\..\"")).unwrap();
    assert!(matches!(
        migrate_error(fx.plan(DISCO)),
        MigrateError::BadPlayTask { .. }
    ));
    // The folder itself isn't a program.
    for exe in [".", "./", ".\\\\"] {
        fs::write(
            &info,
            text.replace("\"Disco Elysium.exe\"", &format!("\"{exe}\"")),
        )
        .unwrap();
        assert!(
            matches!(
                migrate_error(fx.plan(DISCO)),
                MigrateError::BadPlayTask { .. }
            ),
            "{exe}"
        );
    }
    fs::write(&info, format!(r#"{{"gameId":"{DISCO}","playTasks":[]}}"#)).unwrap();
    assert!(matches!(
        migrate_error(fx.plan(DISCO)),
        MigrateError::NoPlayTask { .. }
    ));
    heroic.assert_unchanged();
}

#[test]
fn already_migrated_is_a_no_op() {
    let mut fx = Fixture::new();
    let folder = fx.rimworld();
    fx.native("rimworld", &folder.join("start.sh"), "");
    let migration = fx.migration(RIMWORLD);
    fx.migrated(&migration);
    let heroic = fx.heroic_files();
    let environments = Frozen::of(&fx.paths.environments_dir());
    let states = Frozen::of(&fx.paths.gog_folders_dir());

    match fx.plan(RIMWORLD).unwrap() {
        Plan::AlreadyMigrated {
            environment,
            folder: found,
        } => {
            assert_eq!(environment, "rimworld");
            assert_eq!(found, folder::canonical(&folder).unwrap());
        }
        other => panic!("expected already migrated, got {other:?}"),
    }
    // A plan made before the first apply finds it done as well.
    assert!(matches!(
        fx.apply(&migration).unwrap(),
        Outcome::AlreadyMigrated { environment, .. } if environment == "rimworld"
    ));
    environments.assert_unchanged();
    states.assert_unchanged();
    heroic.assert_unchanged();
}

#[test]
fn a_crash_between_config_and_state_is_finished_by_migrating_again() {
    let mut fx = Fixture::new();
    let folder = fx.rimworld();
    fx.native("rimworld", &folder.join("start.sh"), "");
    let heroic = fx.heroic_files();

    let migration = fx.migration(RIMWORLD);
    set_failpoint(Some("write.before"));
    assert!(fx.apply(&migration).is_err());
    set_failpoint(None);
    assert!(
        fx.config("rimworld")
            .contains(&format!("gog_id = \"{RIMWORLD}\""))
    );
    assert!(fx.state(&folder).is_none());

    let again = fx.migration(RIMWORLD);
    assert!(matches!(
        again.target(),
        Target::Existing {
            records_gog_id: false,
            ..
        }
    ));
    let migrated = fx.migrated(&again);
    assert!(!migrated.recorded_gog_id);
    assert_eq!(
        fx.state(&folder).unwrap().owner.as_deref(),
        Some("rimworld")
    );
    heroic.assert_unchanged();
}

#[test]
fn a_crash_after_creating_an_environment_is_finished_by_migrating_again() {
    let mut fx = Fixture::new();
    let folder = fx.rimworld();
    let heroic = fx.heroic_files();

    let migration = fx.migration(RIMWORLD);
    let Target::Native(plan) = migration.target() else {
        panic!("expected a new native environment");
    };
    assert_eq!(plan.name(), "rimworld");
    set_failpoint(Some("write.before"));
    assert!(fx.apply(&migration).is_err());
    set_failpoint(None);
    let config = fx.config("rimworld");
    assert!(config.contains(&format!("gog_id = \"{RIMWORLD}\"")));
    assert!(config.contains("start.sh"));
    assert!(fx.state(&folder).is_none());

    let again = fx.migration(RIMWORLD);
    assert!(matches!(
        again.target(),
        Target::Existing { name, records_gog_id: false } if name == "rimworld"
    ));
    fx.migrated(&again);
    assert_eq!(fx.config("rimworld"), config);
    let state = fx.state(&folder).unwrap();
    assert_eq!(state.owner.as_deref(), Some("rimworld"));
    assert_eq!(state.committed.unwrap().products.len(), 4);
    heroic.assert_unchanged();
}

#[test]
fn an_ownerless_folder_is_reclaimed_with_its_journal() {
    let mut fx = Fixture::new();
    let folder = fx.rimworld();
    fx.native("rimworld", &folder.join("start.sh"), "");
    let mut existing = FolderState::new(folder::canonical(&folder).unwrap(), None);
    existing.device = Some(7);
    existing.committed = Some(Committed {
        gog_id: 1094900565,
        platform: Platform::Linux,
        language: "de".to_owned(),
        track: Track::Installer,
        products: products(&[(1094900565, Some("1.5"), None)]),
        files: Vec::new(),
    });
    existing.journal = Some(Journal {
        operation: "op-1".to_owned(),
        kind: JournalKind::Update,
        target: JournalTarget {
            gog_id: 1094900565,
            platform: Platform::Linux,
            language: "de".to_owned(),
            track: Track::Installer,
            products: products(&[(1094900565, Some("1.6"), None)]),
        },
        packages: Vec::new(),
        attempted: Vec::new(),
    });
    fx.write_state(&existing);
    let heroic = fx.heroic_files();

    let migration = fx.migration(RIMWORLD);
    assert_eq!(migration.state(), StateChange::Reclaim { journal: true });
    let migrated = fx.migrated(&migration);
    assert_eq!(migrated.state, StateChange::Reclaim { journal: true });
    existing.owner = Some("rimworld".to_owned());
    assert_eq!(fx.state(&folder).unwrap(), existing);
    heroic.assert_unchanged();
}

#[test]
fn a_folder_owned_by_another_environment_or_game_is_refused() {
    let mut fx = Fixture::new();
    let folder = fx.rimworld();
    let before = fx.native("rimworld", &folder.join("start.sh"), "");
    let mut other = FolderState::new(
        folder::canonical(&folder).unwrap(),
        Some("someone".to_owned()),
    );
    other.committed = Some(fx.rimworld_committed(1094900565));
    fx.write_state(&other);
    let heroic = fx.heroic_files();

    assert!(matches!(
        folder_error(fx.plan(RIMWORLD)),
        FolderError::OwnedByOther { owner, .. } if owner == "someone"
    ));

    other.owner = None;
    other.committed = Some(fx.rimworld_committed(5));
    fx.write_state(&other);
    assert!(matches!(
        migrate_error(fx.plan(RIMWORLD)),
        MigrateError::StateOtherGame { other, .. } if other == "5"
    ));
    assert_eq!(fx.config("rimworld"), before);
    heroic.assert_unchanged();
}

#[test]
fn a_folder_inside_or_around_another_game_is_refused() {
    let mut fx = Fixture::new();
    let folder = fx.rimworld_in("Games/Outer/RimWorld");
    fx.native("rimworld", &folder.join("start.sh"), "");
    let heroic = fx.heroic_files();
    for other in ["Games/Outer", "Games/Outer/RimWorld/Inner"] {
        let other = fx.path(other);
        fs::create_dir_all(&other).unwrap();
        let other = folder::canonical(&other).unwrap();
        let mut state = FolderState::new(other.clone(), None);
        state.committed = Some(fx.rimworld_committed(5));
        fx.write_state(&state);
        assert!(matches!(
            folder_error(fx.plan(RIMWORLD)),
            FolderError::Overlap { other_folder, .. } if other_folder == other
        ));
        assert!(folder::forget(&fx.paths, &other, &|| false).unwrap());
    }
    assert!(!fx.config("rimworld").contains("gog_id"));
    heroic.assert_unchanged();
}

#[test]
fn concurrent_adoptions_of_nested_folders_cannot_both_succeed() {
    let mut fx = Fixture::new();
    let outer = fx.rimworld_in("Games/Outer");
    let inner = fx.disco_in("Games/Outer/Disco", "1");
    fx.native("rimworld", &outer.join("start.sh"), "");
    let mut disco = fx.migration(DISCO);
    disco
        .plan_runner(
            &fx.paths,
            &fx.global,
            "fake",
            Some("disco".to_owned()),
            None,
        )
        .unwrap();
    let rimworld = fx.migration(RIMWORLD);
    // Each was planned before the other's state existed.
    assert_eq!(disco.state(), StateChange::Adopt);
    assert_eq!(rimworld.state(), StateChange::Adopt);
    let heroic = fx.heroic_files();

    // A migration waits for the adoption lock, and can be canceled while
    // it does.
    let held = AdoptionLock::acquire(&fx.paths, &|| false).unwrap();
    let token = Token::new();
    token.cancel();
    assert!(matches!(
        folder_error(apply(
            &fx.paths,
            &fx.global,
            &fx.source,
            &rimworld,
            &token,
            |_| {}
        )),
        FolderError::Canceled
    ));

    let fx = Arc::new(fx);
    let start = Arc::new(Barrier::new(3));
    let threads: Vec<_> = [rimworld, disco]
        .into_iter()
        .map(|migration| {
            let (fx, start) = (Arc::clone(&fx), Arc::clone(&start));
            std::thread::spawn(move || {
                start.wait();
                fx.apply(&migration).map(drop)
            })
        })
        .collect();
    start.wait();
    std::thread::sleep(std::time::Duration::from_millis(100));
    drop(held);
    let results: Vec<Result<()>> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let refused = results.into_iter().find_map(Result::err).unwrap();
    assert!(
        matches!(
            refused.downcast_ref::<FolderError>(),
            Some(FolderError::Overlap { .. })
        ),
        "{refused:#}"
    );
    assert_eq!(
        usize::from(fx.state(&outer).is_some()) + usize::from(fx.state(&inner).is_some()),
        1
    );
    // The refused one created nothing.
    let disco_created = fx.paths.environment_config("disco").exists();
    assert_eq!(disco_created, fx.state(&inner).is_some());
    heroic.assert_unchanged();
}

#[test]
fn a_running_environment_or_a_folder_in_use_is_refused() {
    let mut fx = Fixture::new();
    let folder = fx.rimworld();
    let before = fx.native("rimworld", &folder.join("start.sh"), "");
    let heroic = fx.heroic_files();

    let migration = fx.migration(RIMWORLD);
    let run = Runtime::new(&fx.paths, "rimworld")
        .try_lock()
        .unwrap()
        .unwrap();
    // A run also holds the operation lock shared for as long as it lasts.
    let claim = Locks::new(&fx.paths)
        .acquire(Operation::Launch, "rimworld", &[])
        .unwrap();
    for error in [
        fx.plan(RIMWORLD).unwrap_err(),
        fx.apply(&migration).unwrap_err(),
    ] {
        assert_eq!(
            error.to_string(),
            "rimworld is running. Stop it before migrating."
        );
    }
    drop(run);
    let error = fx.apply(&migration).unwrap_err();
    assert!(matches!(
        error.downcast_ref::<MigrateError>(),
        Some(MigrateError::Running { name }) if name == "rimworld"
    ));
    drop(claim);
    // The run lock alone, between a launch's steps, is refused the same way.
    let run = Runtime::new(&fx.paths, "rimworld")
        .try_lock()
        .unwrap()
        .unwrap();
    assert_eq!(
        fx.apply(&migration).unwrap_err().to_string(),
        "rimworld is running. Stop it before migrating."
    );
    drop(run);

    let canonical = folder::canonical(&folder).unwrap();
    let in_use = InUseLock::new(&fx.paths, &canonical)
        .unwrap()
        .try_shared("rimworld-copy")
        .unwrap()
        .unwrap();
    assert!(matches!(
        folder_error(fx.apply(&migration)),
        FolderError::InUse { name, .. } if name == "rimworld"
    ));
    drop(in_use);
    assert_eq!(fx.config("rimworld"), before);
    assert!(fx.state(&folder).is_none());

    // A new environment is refused before it's created.
    fs::remove_file(fx.paths.environment_config("rimworld")).unwrap();
    let migration = fx.migration(RIMWORLD);
    let _in_use = InUseLock::new(&fx.paths, &canonical)
        .unwrap()
        .try_shared("rimworld-copy")
        .unwrap()
        .unwrap();
    assert!(matches!(
        folder_error(fx.apply(&migration)),
        FolderError::InUse { .. }
    ));
    assert!(!fx.paths.environment_config("rimworld").exists());
    heroic.assert_unchanged();
}

#[test]
fn a_migration_refused_for_something_running_says_migrating() {
    let mut fx = Fixture::new();
    let folder = fx.rimworld();
    fx.native("rimworld", &folder.join("start.sh"), "");
    let migration = fx.migration(RIMWORLD);
    let canonical = folder::canonical(&folder).unwrap();
    let lock = InUseLock::new(&fx.paths, &canonical).unwrap();
    let _copy = lock.try_shared("rimworld-copy").unwrap().unwrap();
    assert_eq!(
        fx.apply(&migration).unwrap_err().to_string(),
        "rimworld-copy is running from this game folder. Stop it before migrating rimworld."
    );
    let _other = lock.try_shared("other").unwrap().unwrap();
    assert_eq!(
        fx.apply(&migration).unwrap_err().to_string(),
        "other and rimworld-copy are running from this game folder. Stop them before migrating rimworld."
    );
}

#[test]
fn a_portable_root_is_refused() {
    let mut fx = Fixture::new();
    let folder = fx.rimworld();
    fx.native("rimworld", &folder.join("start.sh"), "");
    let migration = fx.migration(RIMWORLD);
    let heroic = fx.heroic_files();
    let home = fx.root.path().to_owned();
    let portable = Paths::select(
        move |key| (key == "HOME").then(|| home.clone().into_os_string()),
        Some(&fx.path("usb")),
    )
    .unwrap();
    let message = "GOG games live in the host library. Run this without --portable.";
    assert_eq!(
        plan(&portable, &fx.source, RIMWORLD)
            .unwrap_err()
            .to_string(),
        message
    );
    let error = apply(
        &portable,
        &fx.global,
        &fx.source,
        &migration,
        &Token::new(),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(error.to_string(), message);
    assert!(fx.state(&folder).is_none());
    assert!(!fx.config("rimworld").contains("gog_id"));
    heroic.assert_unchanged();
}

#[test]
fn a_game_heroic_has_not_installed_is_its_own_outcome() {
    let mut fx = Fixture::new();
    fx.rimworld();
    let heroic = fx.heroic_files();
    assert!(matches!(
        fx.plan("1207658930").unwrap(),
        Plan::NotInstalled { gog_id } if gog_id == "1207658930"
    ));
    assert!(!fx.paths.gog_folders_dir().exists());
    heroic.assert_unchanged();
}

#[test]
fn a_change_after_planning_is_refused() {
    let mut fx = Fixture::new();
    let folder = fx.rimworld();
    fx.native("rimworld", &folder.join("start.sh"), "");
    let heroic = fx.heroic_files();
    let migration = fx.migration(RIMWORLD);
    fs::write(
        folder.join("gameinfo"),
        format!("RimWorld\n1.6.5000\n1\nen-US\n{RIMWORLD}\n"),
    )
    .unwrap();
    assert!(matches!(
        migrate_error(fx.apply(&migration)),
        MigrateError::Changed
    ));
    assert!(!fx.config("rimworld").contains("gog_id"));
    heroic.assert_unchanged();
}

#[test]
fn an_environment_owning_another_folder_for_the_game_is_already_migrated() {
    let mut fx = Fixture::new();
    let folder = fx.rimworld();
    let before = fx.native(
        "rimworld",
        &folder.join("start.sh"),
        &format!("gog_id = \"{RIMWORLD}\"\n"),
    );
    let elsewhere = fx.path("Elsewhere/RimWorld");
    fs::create_dir_all(&elsewhere).unwrap();
    let mut state = FolderState::new(
        folder::canonical(&elsewhere).unwrap(),
        Some("rimworld".to_owned()),
    );
    state.committed = Some(fx.rimworld_committed(1094900565));
    fx.write_state(&state);
    let heroic = fx.heroic_files();
    // The outcome names the folder it owns, which isn't Heroic's.
    assert!(matches!(
        fx.plan(RIMWORLD).unwrap(),
        Plan::AlreadyMigrated { environment, folder } if environment == "rimworld" && folder == state.folder
    ));
    assert_eq!(fx.config("rimworld"), before);
    assert!(fx.state(&folder).is_none());
    heroic.assert_unchanged();
}

#[test]
fn a_new_environment_removed_or_changed_before_its_state_is_written_gets_none() {
    for change in ["rm", "unset", "recreate"] {
        let mut fx = Fixture::new();
        let folder = fx.rimworld();
        let heroic = fx.heroic_files();
        let migration = fx.migration(RIMWORLD);
        let paths = fx.paths.clone();
        let start = folder.join("start.sh");
        on_create(move || {
            let config = paths.environment_config("rimworld");
            match change {
                "rm" => fs::remove_file(&config).unwrap(),
                "unset" => {
                    crate::settings::apply(
                        &paths,
                        crate::settings::ConfigFile::Environment("rimworld"),
                        &[],
                        &["gog_id"],
                    )
                    .unwrap();
                }
                _ => {
                    fs::remove_file(&config).unwrap();
                    fs::write(
                        &config,
                        format!(
                            "kind = \"native\"\n\n[targets.game]\nexe = \"{}\"\n",
                            start.display()
                        ),
                    )
                    .unwrap();
                }
            }
        });
        assert!(
            matches!(migrate_error(fx.apply(&migration)), MigrateError::Changed),
            "{change}"
        );
        assert!(fx.state(&folder).is_none(), "{change}");
        heroic.assert_unchanged();
    }
}

#[test]
fn an_install_path_leading_elsewhere_since_planning_is_refused() {
    let mut fx = Fixture::new();
    let first = fx.rimworld_in("Games/A");
    let second = fx.path("Games/B");
    fs::create_dir_all(&second).unwrap();
    for file in ["gameinfo", "start.sh"] {
        fs::copy(first.join(file), second.join(file)).unwrap();
    }
    let link = fx.path("Games/RimWorld");
    std::os::unix::fs::symlink(&first, &link).unwrap();
    fx.installed[0]["install_path"] = serde_json::json!(link);
    fx.write_installed();
    fx.native("rimworld", &first.join("start.sh"), "");
    let heroic = fx.heroic_files();

    let migration = fx.migration(RIMWORLD);
    assert_eq!(migration.folder(), folder::canonical(&first).unwrap());
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&second, &link).unwrap();
    assert!(matches!(
        migrate_error(fx.apply(&migration)),
        MigrateError::Changed
    ));
    assert!(fx.state(&first).is_none());
    assert!(fx.state(&second).is_none());
    assert!(!fx.config("rimworld").contains("gog_id"));
    heroic.assert_unchanged();
}

#[test]
fn a_windows_environment_is_found_even_when_heroic_s_runner_is_unusable() {
    let mut fx = Fixture::new();
    let folder = fx.disco();
    let settings = fx.heroic().join(format!("GamesConfig/{DISCO}.json"));
    let text = fs::read_to_string(&settings).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
    value[DISCO]["wineVersion"] = serde_json::json!("unusable");
    fs::write(&settings, value.to_string()).unwrap();
    fs::write(fx.heroic().join("config.json"), "{}").unwrap();

    // A new environment needs the runner.
    assert!(fx.plan(DISCO).is_err());

    fx.environment(
        "disco",
        &format!(
            "schema_version = 1\nrunner = \"fake\"\narch = \"win64\"\nprefix = \"{}\"\n\n[targets.disco]\nexe = \"{}\"\ndefault = true\n",
            fx.path("Prefixes/Disco").display(),
            folder.join("Disco Elysium.exe").display()
        ),
    );
    let heroic = fx.heroic_files();
    let migration = fx.migration(DISCO);
    assert!(matches!(
        migration.target(),
        Target::Existing { name, .. } if name == "disco"
    ));
    fx.migrated(&migration);
    assert!(
        fx.config("disco")
            .contains(&format!("gog_id = \"{DISCO}\""))
    );
    assert_eq!(fx.state(&folder).unwrap().owner.as_deref(), Some("disco"));
    heroic.assert_unchanged();
}

#[test]
fn a_config_recording_the_game_that_cannot_be_loaded_is_refused_by_name() {
    let mut fx = Fixture::new();
    let folder = fx.rimworld();
    fx.native("rimworld", &folder.join("start.sh"), "");
    // Valid TOML, but a Wine environment with no arch doesn't load.
    fx.environment(
        "broken",
        &format!(
            "schema_version = 1\nrunner = \"fake\"\nprefix = \"/p\"\ngog_id = \"{RIMWORLD}\"\n"
        ),
    );
    let heroic = fx.heroic_files();
    let error = migrate_error(fx.plan(RIMWORLD));
    assert!(
        matches!(&error, MigrateError::Unloadable { name, .. } if name == "broken"),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .starts_with("broken records GOG game 1094900565 but can't be loaded: ")
    );

    // It counts toward more than one match.
    fx.native(
        "tracked",
        &fx.path("x/run.sh"),
        &format!("gog_id = \"{RIMWORLD}\"\n"),
    );
    assert!(matches!(
        migrate_error(fx.plan(RIMWORLD)),
        MigrateError::Ambiguous { names, .. } if names == ["broken", "tracked"]
    ));
    fs::remove_file(fx.paths.environment_config("tracked")).unwrap();

    // A file that isn't TOML at all can't say which game it is, so it's
    // skipped.
    fx.environment("broken", &format!("gog_id = \"{RIMWORLD}\" [[[ not toml"));
    let migration = fx.migration(RIMWORLD);
    assert!(matches!(
        migration.target(),
        Target::Existing { name, .. } if name == "rimworld"
    ));
    assert!(fx.state(&folder).is_none());
    heroic.assert_unchanged();
}

#[test]
fn the_game_folder_itself_is_a_working_directory() {
    for dir in [".", "./", ".\\\\"] {
        let mut fx = Fixture::new();
        let folder = fx.disco();
        let info = folder.join(format!("goggame-{DISCO}.info"));
        let text = fs::read_to_string(&info).unwrap();
        fs::write(&info, text.replace("\"bin\"", &format!("\"{dir}\""))).unwrap();
        // An existing environment attaches.
        fx.environment(
            "disco",
            &format!(
                "schema_version = 1\nrunner = \"fake\"\narch = \"win64\"\nprefix = \"{}\"\n\n[targets.disco]\nexe = \"{}\"\ndefault = true\n",
                fx.path("Prefixes/Disco").display(),
                folder.join("Disco Elysium.exe").display()
            ),
        );
        let migration = fx.migration(DISCO);
        assert_eq!(
            migration.launch.cwd.as_deref(),
            Some(folder.as_path()),
            "{dir}"
        );
        assert!(matches!(
            migration.target(),
            Target::Existing { name, .. } if name == "disco"
        ));
        fx.migrated(&migration);
        assert_eq!(fx.state(&folder).unwrap().owner.as_deref(), Some("disco"));
    }
}

#[test]
fn a_target_edited_after_matching_is_not_attached() {
    let mut fx = Fixture::new();
    let folder = fx.rimworld();
    fx.native("rimworld", &folder.join("start.sh"), "");
    let heroic = fx.heroic_files();
    let migration = fx.migration(RIMWORLD);
    let paths = fx.paths.clone();
    let other = fx.path("unrelated.sh");
    fs::write(&other, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&other, fs::Permissions::from_mode(0o755)).unwrap();
    // A target edit doesn't take the operation lock, so it can land after
    // matching, while migration holds the folder's locks.
    on("before_record", move || {
        let exe = other.display().to_string();
        crate::settings::apply(
            &paths,
            crate::settings::ConfigFile::Environment("rimworld"),
            &[("targets.game.exe", exe.as_str())],
            &[],
        )
        .unwrap();
    });
    assert!(matches!(
        migrate_error(fx.apply(&migration)),
        MigrateError::Changed
    ));
    let config = fx.config("rimworld");
    assert!(config.contains("unrelated.sh"), "{config}");
    assert!(!config.contains("gog_id"));
    assert!(fx.state(&folder).is_none());

    // An id match is checked by its id: unsetting it meanwhile stops it.
    let mut fx = Fixture::new();
    let folder = fx.rimworld();
    fx.native(
        "tracked",
        &fx.path("elsewhere/run.sh"),
        &format!("gog_id = \"{RIMWORLD}\"\n"),
    );
    let migration = fx.migration(RIMWORLD);
    let config = fx.paths.environment_config("tracked");
    on("before_record", move || {
        let text = fs::read_to_string(&config).unwrap();
        fs::write(
            &config,
            text.replace(&format!("gog_id = \"{RIMWORLD}\"\n"), ""),
        )
        .unwrap();
    });
    assert!(matches!(
        migrate_error(fx.apply(&migration)),
        MigrateError::Changed
    ));
    assert!(fx.state(&folder).is_none());
    heroic.assert_unchanged();
}

#[test]
fn heroic_changing_while_planning_is_refused() {
    let mut fx = Fixture::new();
    let folder = fx.rimworld();
    fx.native("rimworld", &folder.join("start.sh"), "");
    let installed = fx.heroic().join("gog_store/installed.json");
    on("after_reading", move || {
        let text = fs::read_to_string(&installed).unwrap();
        fs::write(&installed, text.replace("1.6.4871 rev588", "1.6.5000")).unwrap();
    });
    assert!(matches!(
        migrate_error(fx.plan(RIMWORLD)),
        MigrateError::Changed
    ));
    // Planned again, it describes what Heroic has now.
    let migration = fx.migration(RIMWORLD);
    assert_eq!(
        migration.committed().products[0].version.as_deref(),
        Some("1.6.5000")
    );

    // The game folder's info file counts too.
    let gameinfo = folder.join("gameinfo");
    on("after_reading", move || {
        let text = fs::read_to_string(&gameinfo).unwrap();
        fs::write(&gameinfo, text.replace("93132", "1")).unwrap();
    });
    assert!(matches!(
        migrate_error(fx.plan(RIMWORLD)),
        MigrateError::Changed
    ));
}

#[test]
fn an_exe_that_leads_to_a_folder_is_refused() {
    let mut fx = Fixture::new();
    let folder = fx.disco();
    let heroic = fx.heroic_files();
    let info = folder.join(format!("goggame-{DISCO}.info"));
    let text = fs::read_to_string(&info).unwrap();
    std::os::unix::fs::symlink(".", folder.join("self.exe")).unwrap();
    fs::write(&info, text.replace("\"Disco Elysium.exe\"", "\"self.exe\"")).unwrap();
    assert!(matches!(
        migrate_error(fx.plan(DISCO)),
        MigrateError::BadPlayTask { .. }
    ));
    // So is a plain folder.
    fs::write(&info, text.replace("\"Disco Elysium.exe\"", "\"bin\"")).unwrap();
    assert!(matches!(
        migrate_error(fx.plan(DISCO)),
        MigrateError::BadPlayTask { .. }
    ));
    // A missing exe is still left for the launch to report.
    fs::write(
        &info,
        text.replace("\"Disco Elysium.exe\"", "\"missing.exe\""),
    )
    .unwrap();
    assert!(matches!(fx.plan(DISCO).unwrap(), Plan::Migrate(_)));
    heroic.assert_unchanged();
}
