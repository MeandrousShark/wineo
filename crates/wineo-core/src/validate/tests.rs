use crate::test_script::write_script;
use std::ffi::OsString;
use std::os::unix::fs::symlink;

use jiff::Timestamp;

use super::*;
use crate::runtime::{OperationState, RunMode, RunState, RunStatus};

fn apply(
    paths: &Paths,
    fix: &Fix,
    download: impl FnOnce(Component, Option<&str>) -> anyhow::Result<()>,
) -> anyhow::Result<Vec<Note>> {
    super::apply(
        paths,
        fix,
        &crate::cancel::Token::new(),
        winetricks::EnsureOutput::Stderr,
        |_| {},
        download,
    )
}

#[test]
fn report_fixes_deduplicates_shared_downloads_in_problem_order() {
    let download = Fix::Download {
        component: Component::Dxvk,
        version: Some("2.7".to_owned()),
    };
    let cleanup = Fix::RemoveStateDir {
        environment: "oldgame".to_owned(),
    };
    let other = Fix::Download {
        component: Component::Dxvk,
        version: Some("2.6".to_owned()),
    };
    let report = Report {
        problems: [
            Some(download.clone()),
            None,
            Some(cleanup.clone()),
            Some(download.clone()),
            Some(other.clone()),
            Some(cleanup.clone()),
        ]
        .into_iter()
        .map(|fix| Problem::new(Code::BuildMissing, None, "problem".to_owned(), fix))
        .collect(),
        ..Report::default()
    };
    assert_eq!(report.fixes(), [download, cleanup, other]);
    assert!(Report::default().fixes().is_empty());
}

#[test]
fn a_winetricks_fix_with_log_output_reports_progress() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    fs::create_dir(&bin).unwrap();
    executable(
        &bin.join("winetricks"),
        "#!/bin/sh\nfor verb in \"$@\"; do\n  case \"$verb\" in -q) ;; *) echo \"Executing w_do_call $verb\" >&2 ;; esac\ndone\n",
    );
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "validate::tests::winetricks_fix_log_child",
            "--nocapture",
        ])
        .env("WINEO_VALIDATE_LOG_TEST", "1")
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn winetricks_fix_log_child() {
    if std::env::var_os("WINEO_VALIDATE_LOG_TEST").is_none() {
        return;
    }
    let f = Fixture::new();
    f.wine("game", "winetricks = ['corefonts', 'gdiplus']\n");
    f.global("");
    let log = f.root.join("install.log");
    fs::write(&log, "stale output\n").unwrap();
    let mut started = 0;
    let mut steps = Vec::new();
    let mut lines = Vec::new();
    super::apply(
        &f.paths,
        &Fix::InstallWinetricks {
            environment: "game".to_owned(),
        },
        &crate::cancel::Token::new(),
        winetricks::EnsureOutput::Log(&log),
        |event| match event {
            winetricks::WinetricksEvent::LogStarted => {
                started += 1;
                assert!(!fs::read_to_string(&log).unwrap().contains("stale output"));
            }
            winetricks::WinetricksEvent::Progress(step) => {
                assert_eq!(started, 1);
                steps.push(step);
            }
            winetricks::WinetricksEvent::OutputLine(line) => {
                assert_eq!(started, 1);
                lines.push(line);
            }
            _ => {}
        },
        no_download,
    )
    .unwrap();
    assert_eq!(started, 1);
    assert_eq!(
        steps,
        [
            winetricks::Step {
                verb: "corefonts".to_owned(),
                position: 1,
                total: 2
            },
            winetricks::Step {
                verb: "gdiplus".to_owned(),
                position: 2,
                total: 2
            },
        ]
    );
    assert_eq!(
        lines,
        [
            "Executing w_do_call corefonts",
            "Executing w_do_call gdiplus"
        ]
    );
    assert_eq!(
        fs::read_to_string(log).unwrap(),
        "Executing w_do_call corefonts\nExecuting w_do_call gdiplus\n"
    );
}

const HEALTHY: &str = "[targets.game]\nexe = 'C:\\Game\\game.exe'\n";

struct Fixture {
    _root: tempfile::TempDir,
    root: PathBuf,
    paths: Paths,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let paths = Self::xdg(&root, None);
        let fixture = Self {
            _root: dir,
            root,
            paths,
        };
        fixture.runner("fake");
        fixture
    }

    fn xdg(root: &Path, portable: Option<&Path>) -> Paths {
        let root = root.to_owned();
        Paths::select(
            move |key| {
                let path = match key {
                    "HOME" => root.clone(),
                    "XDG_CONFIG_HOME" => root.join("config"),
                    "XDG_DATA_HOME" => root.join("data"),
                    "XDG_STATE_HOME" => root.join("state"),
                    _ => return None,
                };
                Some(OsString::from(path.as_os_str()))
            },
            portable,
        )
        .unwrap()
    }

    /// A wine build of stub scripts, enough to resolve without wine.
    fn runner(&self, name: &str) {
        let bin = self.paths.default_runner_root().join(name).join("bin");
        fs::create_dir_all(&bin).unwrap();
        for program in ["wine", "wineserver"] {
            executable(&bin.join(program), "#!/bin/sh\nexit 0\n");
        }
    }

    fn prefix_dir(&self, name: &str) -> PathBuf {
        self.paths.default_prefix_root().join(name)
    }

    /// A wine prefix holding `C:\Game\game.exe`.
    fn prefix(&self, name: &str) -> PathBuf {
        let prefix = self.prefix_dir(name);
        fs::create_dir_all(prefix.join("drive_c/Game")).unwrap();
        fs::create_dir_all(prefix.join("dosdevices")).unwrap();
        fs::write(prefix.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
        let _ = symlink("../drive_c", prefix.join("dosdevices/c:"));
        fs::write(prefix.join("drive_c/Game/game.exe"), b"").unwrap();
        prefix
    }

    fn write_config(&self, name: &str, text: &str) {
        let path = self.paths.environment_config(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// A Wine environment on the fake runner, its prefix built.
    fn wine(&self, name: &str, rest: &str) {
        self.prefix(name);
        self.wine_at(name, &self.prefix_dir(name), rest);
    }

    fn wine_at(&self, name: &str, prefix: &Path, rest: &str) {
        self.write_config(
            name,
            &format!(
                "prefix = '{}'\nrunner = 'fake'\narch = 'win64'\n{rest}",
                prefix.display()
            ),
        );
    }

    fn native(&self, name: &str, targets: &str) {
        self.write_config(name, &format!("kind = 'native'\n{targets}"));
    }

    fn proxied(&self, name: &str, app_id: &str, desktop: bool) {
        self.write_config(
            name,
            &format!(
                "kind = 'proxied'\nlauncher = 'steam'\napp_id = '{app_id}'\ndesktop = {desktop}\n"
            ),
        );
    }

    fn steam_root(&self) -> PathBuf {
        let root = self.root.join(".local/share/Steam");
        fs::create_dir_all(root.join("steamapps")).unwrap();
        root
    }

    fn steam_manifest(&self, app_id: &str, text: &str) {
        let path = self
            .steam_root()
            .join("steamapps")
            .join(format!("appmanifest_{app_id}.acf"));
        fs::write(path, text).unwrap();
    }

    fn global(&self, text: &str) {
        let path = self.paths.global_config();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn template(&self, name: &str, text: &str) {
        let dir = self.paths.templates_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{name}.toml")), text).unwrap();
    }

    fn check(&self) -> Report {
        check_all(&self.paths).unwrap()
    }

    fn codes(&self) -> Vec<Code> {
        codes(&self.check())
    }

    fn only(&self, code: Code) -> Problem {
        let report = self.check();
        let found: Vec<&Problem> = report.problems.iter().filter(|p| p.code == code).collect();
        assert_eq!(found.len(), 1, "{:#?}", report.problems);
        found[0].clone()
    }

    /// A run whose owner died, in a group that is gone.
    fn lost_run(&self, name: &str) {
        let mut run = RunState::running(
            RunMode::Detached,
            "game".to_owned(),
            4242,
            Some(4242),
            &Default::default(),
        );
        run.native = true;
        run.boot_id = Some("an-earlier-boot".to_owned());
        run.leader_start = Some(1);
        Runtime::new(&self.paths, name).write_state(&run).unwrap();
    }

    fn unresolved_installer(&self, name: &str, group: Option<u32>) {
        let runner = Runners::at(self.paths.default_runner_root())
            .resolve("fake")
            .unwrap();
        let state = OperationState {
            operation: Operation::Install,
            program: "setup.exe".to_owned(),
            runner,
            prefix: self.prefix_dir(name),
            pid: group,
            group,
            boot_id: Some("an-earlier-boot".to_owned()),
            leader_start: Some(1),
            started_at: Timestamp::now(),
        };
        Runtime::new(&self.paths, name)
            .write_operation(&state)
            .unwrap();
    }

    fn art_dir(&self, name: &str) -> PathBuf {
        self.paths.art_dir(name)
    }

    fn stats_log(&self, name: &str) -> PathBuf {
        self.paths.stats_log(name)
    }

    fn stray_entry(&self, environment: &str, target: &str) -> (PathBuf, PathBuf) {
        let entry = self
            .paths
            .applications_dir()
            .join(format!("wineo-{environment}-{target}.desktop"));
        fs::create_dir_all(entry.parent().unwrap()).unwrap();
        fs::write(
                &entry,
                format!(
                    "[Desktop Entry]\nName={target}\nX-Wineo-Environment={environment}\nX-Wineo-Target={target}\n"
                ),
            )
            .unwrap();
        let icon = self.paths.icons_dir().join(format!(
            "hicolor/256x256/apps/wineo-{environment}-{target}.png"
        ));
        fs::create_dir_all(icon.parent().unwrap()).unwrap();
        fs::write(&icon, b"png").unwrap();
        (entry, icon)
    }

    /// Every config file a user writes, with its bytes.
    fn user_configs(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = Vec::new();
        for dir in [self.paths.environments_dir(), self.paths.templates_dir()] {
            for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
                files.push((entry.path(), fs::read(entry.path()).unwrap()));
            }
        }
        if let Ok(bytes) = fs::read(self.paths.global_config()) {
            files.push((self.paths.global_config(), bytes));
        }
        files.sort();
        files
    }
}

fn executable(path: &Path, content: &str) {
    write_script(path, content);
}

fn codes(report: &Report) -> Vec<Code> {
    report.problems.iter().map(|problem| problem.code).collect()
}

fn no_download(_: Component, _: Option<&str>) -> anyhow::Result<()> {
    panic!("nothing should be downloaded")
}

fn fix(problem: &Problem) -> &Fix {
    problem.fix.as_ref().expect("a fix is offered")
}

#[test]
fn a_healthy_setup_has_no_problems() {
    let f = Fixture::new();
    f.wine("game", HEALTHY);
    let tool = f.root.join("tool");
    executable(&tool, "#!/bin/sh\n");
    f.native(
        "tool",
        &format!("[targets.tool]\nexe = '{}'\n", tool.display()),
    );
    f.template("mine", "runner = 'fake'\ngamemode = false\n");

    let report = f.check();
    assert_eq!(report.problems, []);
    assert_eq!(report.environments, ["game", "tool"]);
    assert_eq!(report.templates, ["mine"]);
    assert_eq!((report.errors(), report.warnings()), (0, 0));
}

#[test]
fn a_bad_gog_language_is_a_global_config_error() {
    let f = Fixture::new();
    f.global("[gog]\nlanguage = \"english\"\n");
    assert_eq!(f.codes(), [Code::GlobalConfigInvalid]);
    f.global("[defaults]\ngames_root = \"~/Library\"\n[gog]\nlanguage = \"de-AT\"\n");
    assert_eq!(f.codes(), []);
}

#[test]
fn a_broken_global_config_is_an_error_and_the_rest_is_still_checked() {
    let f = Fixture::new();
    f.wine("game", "[targets.game]\nexe = 'C:\\Game\\missing.exe'\n");
    f.global("stop_timeuot = 3\n");
    assert_eq!(f.codes(), [Code::GlobalConfigInvalid, Code::ExeMissing]);
    let problem = f.only(Code::GlobalConfigInvalid);
    assert!(
        problem
            .message
            .starts_with("invalid config ~/config/wineo/config.toml: "),
        "{}",
        problem.message
    );
    assert_eq!(problem.environment, None);
    assert_eq!(problem.level, Level::Error);
    assert!(
        problem.message.contains("stop_timeuot"),
        "{}",
        problem.message
    );

    f.global("schema_version = 99\n");
    let problem = f.only(Code::GlobalConfigInvalid);
    assert!(problem.message.contains("unsupported schema_version 99"));
}

#[test]
fn templates_are_parsed_and_their_picks_resolved() {
    let f = Fixture::new();
    f.template("broken", "prefix = '/x'\n");
    f.template("typo", "runer = 'fake'\n");
    f.template("gone", "runner = 'nope'\n");
    f.template("nodxvk", "runner = 'fake'\ndxvk = 'latest'\n");
    let report = f.check();
    assert_eq!(
        codes(&report),
        [
            Code::TemplateInvalid,
            Code::TemplateUnresolved,
            Code::TemplateUnresolved,
            Code::TemplateInvalid,
        ]
    );
    assert_eq!(report.errors(), 2);
    assert_eq!(report.warnings(), 2);
    assert!(report.problems[1].message.starts_with("template gone: "));
    assert_eq!(report.problems[1].template.as_deref(), Some("gone"));
    assert_eq!(report.problems[3].template.as_deref(), Some("typo"));
    assert!(report.problems[2].message.starts_with("template nodxvk: "));
    // The shipped template isn't the user's to fix.
    assert_eq!(report.templates, ["broken", "gone", "nodxvk", "typo"]);
}

#[test]
fn a_config_that_fails_to_parse_skips_its_other_checks() {
    let f = Fixture::new();
    f.wine("game", "prefx = 1\n[targets.a]\nexe = '/missing'\n");
    f.lost_run("game");
    let problem = f.only(Code::EnvironmentConfigInvalid);
    assert_eq!(f.codes(), [Code::EnvironmentConfigInvalid]);
    assert_eq!(problem.environment.as_deref(), Some("game"));
    assert!(problem.message.contains("prefx"));
}

#[test]
fn an_interrupted_restore_is_an_error_and_skips_the_rest() {
    let f = Fixture::new();
    f.wine("game", "prefx = 1\n");
    let marker = f.paths.restore_marker("game");
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    fs::write(&marker, "").unwrap();
    assert_eq!(f.codes(), [Code::RestoreInterrupted]);
    let problem = f.only(Code::RestoreInterrupted);
    assert!(problem.message.contains("wineo snapshot recover game"));
    assert_eq!(problem.fix, None);
}

#[test]
fn a_well_formed_restore_marker_offers_only_explicit_recovery() {
    use std::os::unix::fs::MetadataExt;

    let f = Fixture::new();
    f.wine("game", HEALTHY);
    let prefix = f.prefix_dir("game");
    let metadata = fs::metadata(&prefix).unwrap();
    let marker = f.paths.restore_marker("game");
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    let saved_config = fs::read_to_string(f.paths.environment_config("game")).unwrap();
    let contents = serde_json::json!({
        "version": 1,
        "environment": "game",
        "prefix": prefix,
        "staging": prefix.parent().unwrap().join(".wineo-restore-test"),
        "old_directory": { "device": metadata.dev(), "inode": metadata.ino() },
        "old_fingerprint": crate::import::fingerprint(&prefix).unwrap(),
        "old_config": saved_config,
    });
    fs::write(&marker, serde_json::to_vec(&contents).unwrap()).unwrap();
    let problem = f.only(Code::RestoreInterrupted);
    let wanted = Fix::RecoverSnapshot {
        environment: "game".to_owned(),
    };
    assert_eq!(problem.fix, Some(wanted.clone()));
    assert_eq!(
        wanted.label(),
        "Roll back game's interrupted snapshot restore"
    );
    assert!(!wanted.is_batch_safe());
    let report = check_environment(&f.paths, "game").unwrap();
    assert!(report.fixes().is_empty());
    assert!(apply(&f.paths, &wanted, no_download).is_err());
    assert!(marker.exists());
}

#[test]
fn a_runner_that_does_not_resolve_is_an_error_with_a_hint() {
    let f = Fixture::new();
    f.prefix("game");
    f.write_config(
        "game",
        &format!(
            "prefix = '{}'\nrunner = 'nope'\narch = 'win64'\n{HEALTHY}",
            f.prefix_dir("game").display()
        ),
    );
    let problem = f.only(Code::RunnerMissing);
    assert_eq!(problem.level, Level::Error);
    assert_eq!(problem.area, Area::Wine);
    assert_eq!(problem.fix, None);
    assert!(problem.message.contains("wineo download"));
    // The prefix is still there to check `C:\` paths against.
    assert_eq!(f.codes(), [Code::RunnerMissing]);
}

#[test]
fn a_build_that_does_not_resolve_offers_a_download() {
    let f = Fixture::new();
    f.wine(
        "game",
        &format!("dxvk = 'latest'\nvkd3d = 'vkd3d-proton-2.14'\n{HEALTHY}"),
    );
    let report = f.check();
    assert_eq!(codes(&report), [Code::BuildMissing, Code::BuildMissing]);
    assert_eq!(
        report.problems[0].fix,
        Some(Fix::Download {
            component: Component::Dxvk,
            version: None,
        })
    );
    assert_eq!(
        report.problems[1].fix,
        Some(Fix::Download {
            component: Component::Vkd3d,
            version: Some("vkd3d-proton-2.14".to_owned()),
        })
    );
    assert_eq!(fix(&report.problems[0]).label(), "Download the latest DXVK");
}

#[test]
fn the_download_fix_passes_the_pin_and_skips_a_build_already_there() {
    let f = Fixture::new();
    let wanted = Fix::Download {
        component: Component::Dxvk,
        version: Some("dxvk-2.7".to_owned()),
    };
    let mut asked = None;
    apply(&f.paths, &wanted, |component, version| {
        asked = Some((component, version.map(str::to_owned)));
        Ok(())
    })
    .unwrap();
    assert_eq!(asked, Some((Component::Dxvk, Some("dxvk-2.7".to_owned()))));

    let build = f.paths.default_dxvk_root().join("dxvk-2.7");
    for arch in ["x64", "x32"] {
        fs::create_dir_all(build.join(arch)).unwrap();
        fs::write(build.join(arch).join("dxgi.dll"), b"").unwrap();
    }
    apply(&f.paths, &wanted, no_download).unwrap();
}

#[test]
fn a_missing_prefix_start_would_build_is_a_warning_and_skips_windows_paths() {
    let f = Fixture::new();
    f.wine_at(
        "game",
        &f.prefix_dir("game"),
        "[targets.game]\nexe = 'C:\\Game\\missing.exe'\n",
    );
    let problem = f.only(Code::PrefixMissing);
    assert_eq!(problem.level, Level::Warning);
    assert_eq!(f.codes(), [Code::PrefixMissing]);
}

#[test]
fn a_prefix_start_would_refuse_is_an_error() {
    let f = Fixture::new();
    let full = f.root.join("full");
    fs::create_dir_all(&full).unwrap();
    fs::write(full.join("notes.txt"), "mine").unwrap();
    f.wine_at("full", &full, HEALTHY);

    let proton = f.prefix_dir("proton");
    fs::create_dir_all(proton.join("pfx")).unwrap();
    fs::write(proton.join("pfx/system.reg"), "").unwrap();
    f.wine_at("proton", &proton, HEALTHY);

    let empty = f.root.join("mnt/empty");
    fs::create_dir_all(&empty).unwrap();
    f.wine_at("empty", &empty, "");

    let report = f.check();
    assert_eq!(
        codes(&report),
        [
            Code::PrefixRefused,
            Code::PrefixRefused,
            Code::PrefixRefused
        ]
    );
    assert!(report.problems[0].message.contains("wineo setup empty"));
    assert!(report.problems[2].message.contains("Proton"));
}

#[test]
fn missing_exes_and_working_dirs_are_errors() {
    let f = Fixture::new();
    f.wine(
        "game",
        concat!(
            "[targets.windows]\nexe = 'C:\\Game\\missing.exe'\ncwd = 'C:\\nowhere'\n",
            "[targets.unix]\nexe = '/nonexistent/setup.exe'\n",
            "[targets.bare]\nexe = 'notepad'\ncwd = '/nonexistent'\ndefault = true\n",
            "[targets.fine]\nexe = 'C:\\Game\\game.exe'\n",
        ),
    );
    let report = f.check();
    let messages: Vec<(Code, &str)> = report
        .problems
        .iter()
        .map(|problem| (problem.code, problem.message.as_str()))
        .collect();
    assert_eq!(
        messages,
        [
            (
                Code::CwdMissing,
                "target bare: working directory /nonexistent not found"
            ),
            (
                Code::ExeMissing,
                "target unix: /nonexistent/setup.exe not found"
            ),
            (
                Code::ExeMissing,
                "target windows: C:\\Game\\missing.exe not found"
            ),
        ]
    );
    assert!(report.problems.iter().all(|p| p.area == Area::Target));
}

#[test]
fn gamemode_without_gamemoderun_is_an_error() {
    let f = Fixture::new();
    f.wine("game", &format!("gamemode = true\n{HEALTHY}"));
    let installed = runner::find_on_path(launch::GAMEMODE).is_some();
    let expected: &[Code] = if installed {
        &[]
    } else {
        &[Code::GamemodeMissing]
    };
    assert_eq!(f.codes(), expected);
}

#[test]
fn launch_checks_report_missing_wrappers_and_unsupported_wayland() {
    let f = Fixture::new();
    f.wine(
        "game",
        &format!("wayland = true\nmangohud = true\n[gamescope]\n{HEALTHY}"),
    );
    let report = f.check();
    let has = |code| report.problems.iter().find(|p| p.code == code);
    for (code, program) in [
        (Code::GamescopeMissing, "gamescope"),
        (Code::MangohudMissing, "mangoapp"),
    ] {
        if runner::find_on_path(program).is_none() {
            let problem = has(code).unwrap();
            assert!(problem.message.contains(program));
            assert_eq!(problem.area, Area::Target);
            assert_eq!(problem.level, Level::Error);
        }
    }
    let problem = has(Code::WaylandUnsupported).unwrap();
    assert!(problem.message.contains("fake doesn't support it"));
    assert_eq!(problem.area, Area::Target);
    assert_eq!(problem.level, Level::Error);

    f.wine("game", &format!("mangohud = true\n{HEALTHY}"));
    let report = f.check();
    if runner::find_on_path("mangohud").is_none() {
        assert!(report.problems.iter().any(
            |p| p.code == Code::MangohudMissing && p.message.contains("mangohud was not found")
        ));
    }
    assert!(
        !report
            .problems
            .iter()
            .any(|p| p.code == Code::WaylandUnsupported)
    );

    f.write_config(
        "game",
        &format!(
            "prefix = '{}'\nrunner = 'missing'\narch = 'win64'\nwayland = true\n{HEALTHY}",
            f.prefix_dir("game").display()
        ),
    );
    let report = f.check();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.code == Code::RunnerMissing)
    );
    assert!(
        !report
            .problems
            .iter()
            .any(|p| p.code == Code::WaylandUnsupported)
    );
}

#[test]
fn native_targets_are_resolved_like_a_launch() {
    let f = Fixture::new();
    let script = f.root.join("script");
    fs::write(&script, "#!/bin/sh\n").unwrap();
    f.native(
        "tool",
        &format!(
            concat!(
                "[targets.windows]\nexe = 'C:\\Games\\game.exe'\ndefault = true\n",
                "[targets.plain]\nexe = '{}'\n",
                "[targets.gone]\nexe = '/nonexistent/tool'\n",
            ),
            script.display()
        ),
    );
    let report = f.check();
    assert_eq!(
        codes(&report),
        [
            Code::NativeExeInvalid,
            Code::NativeExeInvalid,
            Code::NativeExeInvalid
        ]
    );
    assert!(report.problems[1].message.contains("chmod +x"));
    assert!(report.problems.iter().all(|p| p.area == Area::NativeTarget));
}

#[test]
fn several_targets_without_a_default_is_a_warning() {
    let f = Fixture::new();
    let two = "[targets.a]\nexe = 'notepad'\n[targets.b]\nexe = 'notepad'\n";
    f.wine("game", two);
    let problem = f.only(Code::TargetAmbiguous);
    assert_eq!(problem.level, Level::Warning);
    assert_eq!(problem.area, Area::Environment);

    f.wine("game", &format!("{two}default = true\n"));
    assert_eq!(f.codes(), []);
}

#[test]
fn proxied_validate_reports_steam_not_found_and_a_missing_selected_install() {
    let f = Fixture::new();
    f.proxied("game", "77", false);
    let problem = f.only(Code::SteamNotFound);
    assert_eq!(problem.level, Level::Error);
    assert_eq!(problem.area, Area::Steam);
    assert_eq!(problem.message, "Steam isn't found.");

    f.global("steam = 'flatpak'\n");
    let problem = f.only(Code::SteamInstallMissing);
    assert_eq!(problem.level, Level::Error);
    assert_eq!(problem.area, Area::Steam);
}

#[test]
fn proxied_validate_does_not_report_a_game_that_is_not_installed() {
    let f = Fixture::new();
    f.proxied("game", "77", false);
    f.steam_root();
    assert_eq!(f.codes(), []);
    // Queued, installing, and uninstalling are normal states too.
    for flags in [2, 1026, 4 | 2048] {
        f.steam_manifest(
            "77",
            &format!(
                "\"AppState\" {{ \"appid\" \"77\" \"name\" \"Game\" \"StateFlags\" \"{flags}\" }}"
            ),
        );
        assert_eq!(f.codes(), [], "flags {flags}");
    }
}

#[test]
fn a_not_installed_game_passes_while_an_unrelated_error_still_fails() {
    let f = Fixture::new();
    f.proxied("game", "77", false);
    f.proxied("broken", "78", false);
    f.steam_manifest(
        "78",
        "\"AppState\" { \"appid\" \"78\" \"StateFlags\" \"bad\" }",
    );
    let report = f.check();
    assert_eq!(codes(&report), [Code::SteamDataUnknown]);
    assert_eq!(report.problems[0].environment.as_deref(), Some("broken"));
    assert_eq!(report.errors(), 1);
}

#[test]
fn proxied_validate_warns_when_a_steam_library_is_unavailable() {
    let f = Fixture::new();
    f.proxied("game", "77", false);
    let root = f.steam_root();
    let missing = f.root.join("missing-library");
    fs::write(
        root.join("steamapps/libraryfolders.vdf"),
        format!(
            "\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} }}",
            missing.display()
        ),
    )
    .unwrap();
    let problem = f.only(Code::SteamLibraryUnavailable);
    assert_eq!(problem.level, Level::Warning);
    assert_eq!(problem.area, Area::Steam);
    assert!(problem.message.ends_with("missing-library is unavailable."));
}

#[test]
fn proxied_validate_reports_an_unreadable_manifest() {
    let f = Fixture::new();
    f.proxied("game", "77", false);
    f.steam_manifest(
        "77",
        "\"AppState\" { \"appid\" \"77\" \"StateFlags\" \"bad\" }",
    );
    let problem = f.only(Code::SteamDataUnknown);
    assert_eq!(problem.level, Level::Error);
    assert_eq!(problem.area, Area::Steam);
}

#[test]
fn a_healthy_proxied_environment_validates_cleanly() {
    let f = Fixture::new();
    f.proxied("game", "77", false);
    f.steam_manifest(
        "77",
        "\"AppState\" { \"appid\" \"77\" \"name\" \"Game\" \"StateFlags\" \"4\" }",
    );
    assert_eq!(f.codes(), []);
}

#[test]
fn implicit_steam_target_art_validates_cleanly() {
    let f = Fixture::new();
    f.proxied("game", "77", false);
    f.steam_manifest(
        "77",
        "\"AppState\" { \"appid\" \"77\" \"name\" \"Game\" \"StateFlags\" \"4\" }",
    );
    let art = f.art_dir("game");
    fs::create_dir_all(art.join("steam")).unwrap();
    fs::write(
        art.join("art.toml"),
        "schema_version = 1\n[targets.steam.icon]\nfile = 'icon.png'\nmanual = 'icon.png'\n",
    )
    .unwrap();
    fs::write(art.join("steam/icon.png"), b"icon").unwrap();

    assert_eq!(f.codes(), []);
}

#[test]
fn proxied_validate_checks_pending_desktop_entries_and_fixes_them() {
    let f = Fixture::new();
    f.proxied("game", "77", true);
    let problem = f.only(Code::DesktopOutdated);
    assert_eq!(problem.level, Level::Warning);
    assert_eq!(problem.area, Area::Desktop);
    apply(&f.paths, fix(&problem), no_download).unwrap();
    let entry = f.paths.applications_dir().join("wineo-game-steam.desktop");
    assert!(
        fs::read_to_string(entry)
            .unwrap()
            .contains("Exec=wineo start game\n")
    );
}

#[test]
fn a_lost_run_is_cleaned_up_like_stop_does() {
    let f = Fixture::new();
    f.native("tool", "");
    f.lost_run("tool");
    let problem = f.only(Code::LostRun);
    assert_eq!(problem.level, Level::Warning);
    assert_eq!(fix(&problem).label(), "Clean up tool's lost run");

    apply(&f.paths, fix(&problem), no_download).unwrap();
    let run = Runtime::new(&f.paths, "tool")
        .read_state()
        .unwrap()
        .unwrap();
    assert_eq!(run.status, RunStatus::Stopped);
    assert_eq!(f.codes(), []);
}

#[test]
fn a_lost_run_reports_when_its_session_cannot_be_logged() {
    let f = Fixture::new();
    f.native("tool", "");
    f.lost_run("tool");
    fs::create_dir_all(f.paths.stats_log("tool")).unwrap();
    let problem = f.only(Code::LostRun);

    let notes = apply(&f.paths, fix(&problem), no_download).unwrap();
    assert_eq!(notes.len(), 1);
    assert!(matches!(&notes[0], Note::Warning(message) if message.starts_with("failed to open ")));
    let run = Runtime::new(&f.paths, "tool")
        .read_state()
        .unwrap()
        .unwrap();
    assert_eq!(run.status, RunStatus::Stopped);
}

#[test]
fn an_unresolved_installer_is_cleaned_up_like_stop_does() {
    let f = Fixture::new();
    f.wine("game", HEALTHY);
    f.unresolved_installer("game", Some(4242));
    let problem = f.only(Code::InstallerUnresolved);
    assert_eq!(problem.level, Level::Warning);
    apply(&f.paths, fix(&problem), no_download).unwrap();
    assert!(!Runtime::new(&f.paths, "game").operation_path().exists());
    assert_eq!(f.codes(), []);
}

#[test]
fn an_installer_cleanup_that_cannot_be_confirmed_fails_and_keeps_the_record() {
    let f = Fixture::new();
    f.wine("game", HEALTHY);
    f.unresolved_installer("game", None);
    let problem = f.only(Code::InstallerUnresolved);
    let err = apply(&f.paths, fix(&problem), no_download).unwrap_err();
    assert!(
        err.to_string().contains("could not confirm cleanup"),
        "{err}"
    );
    assert!(Runtime::new(&f.paths, "game").operation_path().exists());
}

#[test]
fn a_state_dir_without_an_environment_is_removed() {
    let f = Fixture::new();
    f.wine("game", HEALTHY);
    let stray = f.paths.runtime_dir("oldgame");
    fs::create_dir_all(&stray).unwrap();
    fs::write(stray.join("output.log"), "log").unwrap();
    // wineo's own dirs aren't environments.
    fs::create_dir_all(f.paths.locks_dir()).unwrap();
    fs::create_dir_all(f.paths.import_log("x").parent().unwrap()).unwrap();
    fs::create_dir_all(f.paths.runtime_dir("imports")).unwrap();
    fs::create_dir_all(f.paths.runtime_dir("game")).unwrap();

    let problem = f.only(Code::StateDirStray);
    assert_eq!(f.codes(), [Code::StateDirStray]);
    assert_eq!(
        problem.message,
        "state dir ~/state/wineo/oldgame has no environment"
    );
    assert_eq!(problem.environment, None);
    assert_eq!(problem.stray.as_deref(), Some("oldgame"));
    assert_eq!(fix(&problem).label(), "Remove oldgame's leftover state");
    apply(&f.paths, fix(&problem), no_download).unwrap();
    assert!(!stray.exists());
    assert_eq!(f.codes(), []);
}

#[test]
fn a_stats_log_without_an_environment_is_removed() {
    let f = Fixture::new();
    f.wine("game", HEALTHY);
    let stray = f.stats_log("oldgame");
    fs::create_dir_all(stray.parent().unwrap()).unwrap();
    fs::write(&stray, "{}\n").unwrap();
    // A live environment's own log isn't a stray.
    fs::write(f.stats_log("game"), "{}\n").unwrap();

    let problem = f.only(Code::StatsStray);
    assert_eq!(f.codes(), [Code::StatsStray]);
    assert_eq!(problem.area, Area::Runtime);
    assert!(problem.message.contains("play stats"));
    assert!(problem.message.contains("has no environment"));
    assert_eq!(problem.environment, None);
    assert_eq!(problem.stray.as_deref(), Some("oldgame"));
    assert_eq!(
        fix(&problem).label(),
        "Remove oldgame's leftover play stats"
    );
    apply(&f.paths, fix(&problem), no_download).unwrap();
    assert!(!stray.exists());
    assert_eq!(f.codes(), []);
}

#[test]
fn a_state_dir_still_tracking_a_run_is_kept() {
    let f = Fixture::new();
    f.lost_run("oldgame");
    let problem = f.only(Code::StateDirStray);
    let err = apply(&f.paths, fix(&problem), no_download).unwrap_err();
    assert!(err.to_string().contains("last run was lost"), "{err}");
    assert!(f.paths.runtime_dir("oldgame").exists());
}

#[test]
fn a_desktop_entry_without_an_environment_is_removed_with_its_icons() {
    let f = Fixture::new();
    let (entry, icon) = f.stray_entry("oldgame", "oldgame");
    let problem = f.only(Code::DesktopEntryStray);
    assert_eq!(
        problem.message,
        "desktop entry wineo-oldgame-oldgame.desktop has no environment"
    );
    assert_eq!(problem.stray.as_deref(), Some("oldgame"));
    apply(&f.paths, fix(&problem), no_download).unwrap();
    assert!(!entry.exists());
    assert!(!icon.exists());
    assert_eq!(f.codes(), []);
}

#[test]
fn out_of_date_desktop_entries_are_rewritten() {
    let f = Fixture::new();
    f.wine("game", &format!("{HEALTHY}desktop = true\n"));
    let problem = f.only(Code::DesktopOutdated);
    assert_eq!(
        problem.message,
        "desktop entries for game are out of date with the config"
    );
    let entry = f.paths.applications_dir().join("wineo-game-game.desktop");
    assert!(!entry.exists());

    apply(&f.paths, fix(&problem), no_download).unwrap();
    assert!(entry.exists());
    assert_eq!(f.codes(), []);

    f.wine("game", HEALTHY);
    assert_eq!(f.codes(), [Code::DesktopOutdated]);
    apply(&f.paths, fix(&f.only(Code::DesktopOutdated)), no_download).unwrap();
    assert!(!entry.exists());
}

#[test]
fn a_native_environments_entries_are_checked_too() {
    let f = Fixture::new();
    let tool = f.root.join("tool");
    executable(&tool, "#!/bin/sh\n");
    f.native(
        "tool",
        &format!(
            "[targets.tool]\nexe = '{}'\ndesktop = true\n",
            tool.display()
        ),
    );
    let problem = f.only(Code::DesktopOutdated);
    apply(&f.paths, fix(&problem), no_download).unwrap();
    assert_eq!(f.codes(), []);
}

#[test]
fn checking_every_environment_reads_the_desktop_entries_once() {
    let f = Fixture::new();
    for index in 0..6 {
        f.wine(
            &format!("game{index}"),
            &format!("{HEALTHY}desktop = true\n"),
        );
    }
    f.stray_entry("oldgame", "oldgame");
    desktop::LISTS.set(0);
    let report = f.check();
    assert_eq!(desktop::LISTS.get(), 1);
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.code == Code::DesktopEntryStray)
    );
}

#[test]
fn portable_mode_checks_only_its_own_root_and_skips_desktop_checks() {
    let f = Fixture::new();
    f.stray_entry("oldgame", "oldgame");
    let usb = f.root.join("usb");
    let paths = Fixture::xdg(&f.root, Some(&usb));
    let config = paths.environment_config("game");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
            &config,
            "prefix = 'prefixes/game'\nrunner = 'fake'\narch = 'win64'\n[targets.game]\nexe = 'notepad'\ndesktop = true\n",
        )
        .unwrap();
    let report = check_all(&paths).unwrap();
    assert_eq!(codes(&report), [Code::PrefixMissing, Code::DesktopPortable]);
    assert_eq!(report.environments, ["game"]);
}

#[test]
fn art_records_whose_file_or_target_is_gone_are_dropped() {
    let f = Fixture::new();
    f.wine("game", HEALTHY);
    let art = f.art_dir("game");
    fs::create_dir_all(&art).unwrap();
    let text = concat!(
        "# kept by hand\n",
        "schema_version = 1\n\n",
        "[icon] # mine\n",
        "file = \"icon.png\"\n",
        "manual = \"icon.png\"\n\n",
        "[targets.old.grid]\n",
        "file = \"grid.png\"\n",
        "manual = \"grid.png\"\n",
    );
    fs::write(art.join("art.toml"), text).unwrap();

    let report = f.check();
    assert_eq!(
        codes(&report),
        [Code::ArtFileMissing, Code::ArtTargetUnknown]
    );
    assert_eq!(
        report.problems[0].message,
        "art: icon.png is recorded but missing"
    );
    assert_eq!(
        fix(&report.problems[0]).label(),
        "Drop game's missing icon record"
    );
    assert_eq!(
        fix(&report.problems[1]).label(),
        "Drop game's art records for old"
    );
    for problem in &report.problems {
        apply(&f.paths, fix(problem), no_download).unwrap();
    }
    assert_eq!(f.codes(), []);
    let after = fs::read_to_string(art.join("art.toml")).unwrap();
    assert_eq!(after, "# kept by hand\nschema_version = 1\n");
}

#[test]
fn a_dropped_record_whose_file_came_back_is_kept() {
    let f = Fixture::new();
    f.wine("game", HEALTHY);
    let art = f.art_dir("game");
    fs::create_dir_all(&art).unwrap();
    let text = "schema_version = 1\n[icon]\nfile = \"icon.png\"\nmanual = \"a.png\"\n";
    fs::write(art.join("art.toml"), text).unwrap();
    let problem = f.only(Code::ArtFileMissing);
    fs::write(art.join("icon.png"), b"png").unwrap();
    apply(&f.paths, fix(&problem), no_download).unwrap();
    assert_eq!(fs::read_to_string(art.join("art.toml")).unwrap(), text);
}

#[test]
fn duplicate_art_stray_art_dirs_and_a_broken_art_toml_are_reported() {
    let f = Fixture::new();
    f.wine("game", HEALTHY);
    f.wine("other", HEALTHY);
    let art = f.art_dir("game");
    fs::create_dir_all(&art).unwrap();
    fs::write(art.join("icon.png"), b"png").unwrap();
    fs::write(art.join("icon.ico"), b"ico").unwrap();
    fs::create_dir_all(f.art_dir("oldgame")).unwrap();
    fs::create_dir_all(f.art_dir(".wineo-art-staging")).unwrap();
    fs::create_dir_all(f.art_dir("other")).unwrap();
    fs::write(f.art_dir("other").join("art.toml"), "icon = 3\n").unwrap();

    let report = f.check();
    assert_eq!(
        codes(&report),
        [
            Code::ArtDuplicates,
            Code::ArtConfigInvalid,
            Code::ArtDirStray
        ]
    );
    assert_eq!(
        report.problems[0].message,
        "art: icon.png is used, so icon.ico is ignored"
    );
    assert_eq!(report.problems[0].fix, None);
    assert_eq!(report.problems[1].level, Level::Error);
    assert_eq!(report.problems[2].fix, None);
    assert_eq!(report.problems[2].stray.as_deref(), Some("oldgame"));
}

#[test]
fn a_fix_on_a_busy_environment_fails_without_stopping_the_others() {
    let f = Fixture::new();
    f.native("tool", "");
    f.lost_run("tool");
    f.native("idle", "");
    f.lost_run("idle");
    fs::create_dir_all(f.paths.runtime_dir("oldgame")).unwrap();
    f.stray_entry("gone", "gone");

    let locks = Locks::new(&f.paths);
    let _tool = locks.acquire(Operation::Create, "tool", &[]).unwrap();
    let _oldgame = locks.acquire(Operation::Snapshot, "oldgame", &[]).unwrap();

    let report = f.check();
    let fixes: Vec<&Fix> = report
        .problems
        .iter()
        .filter_map(|p| p.fix.as_ref())
        .collect();
    assert_eq!(fixes.len(), 4);
    let results: Vec<(String, std::result::Result<(), String>)> = fixes
        .into_iter()
        .map(|fix| {
            let result = apply(&f.paths, fix, no_download)
                .map(|_| ())
                .map_err(|err| err.to_string());
            (fix.label(), result)
        })
        .collect();
    assert_eq!(
        results,
        [
            ("Clean up idle's lost run".to_owned(), Ok(())),
            (
                "Clean up tool's lost run".to_owned(),
                Err("environment `tool` is busy: it is being created".to_owned())
            ),
            (
                "Remove oldgame's leftover state".to_owned(),
                Err("environment `oldgame` is busy: a snapshot operation is running".to_owned())
            ),
            ("Remove gone's desktop entry".to_owned(), Ok(())),
        ]
    );
    assert!(f.paths.runtime_dir("oldgame").exists());
    assert!(Runtime::new(&f.paths, "tool").status().unwrap().lost);
}

#[test]
fn fixes_leave_the_users_configs_byte_for_byte_unchanged() {
    let f = Fixture::new();
    f.global("# mine\nstop_timeout = 5 # seconds\n");
    f.template("mine", "# notes\nrunner = 'fake'\n");
    f.wine(
        "game",
        &format!("# a comment\n{HEALTHY}desktop = true   # spacing kept\n"),
    );
    f.lost_run("game");
    f.unresolved_installer("game", Some(4242));
    let art = f.art_dir("game");
    fs::create_dir_all(&art).unwrap();
    fs::write(
            art.join("art.toml"),
            "[icon]\nfile = \"icon.png\"\nmanual = \"a.png\"\n[targets.old.icon]\nfile = \"icon.png\"\nmanual = \"a.png\"\n",
        )
        .unwrap();
    fs::create_dir_all(f.paths.runtime_dir("oldgame")).unwrap();
    f.stray_entry("gone", "gone");
    let before = f.user_configs();

    let report = f.check();
    let fixes: Vec<&Fix> = report
        .problems
        .iter()
        .filter_map(|p| p.fix.as_ref())
        .collect();
    assert_eq!(fixes.len(), 7, "{:#?}", report.problems);
    for fix in fixes {
        apply(&f.paths, fix, no_download).unwrap();
    }
    assert_eq!(f.user_configs(), before);
    assert_eq!(f.codes(), []);
}

#[test]
fn check_environment_checks_only_that_environment() {
    let f = Fixture::new();
    f.global("stop_timeuot = 3\n");
    f.wine("game", "[targets.game]\nexe = 'C:\\Game\\missing.exe'\n");
    f.wine("other", "[targets.game]\nexe = 'C:\\Game\\missing.exe'\n");
    fs::create_dir_all(f.paths.runtime_dir("oldgame")).unwrap();
    let report = check_environment(&f.paths, "game").unwrap();
    assert_eq!(codes(&report), [Code::ExeMissing]);
    assert_eq!(report.environments, ["game"]);
    assert!(matches!(
        check_environment(&f.paths, "nope"),
        Err(Error::UnknownEnvironment(_))
    ));
}

// The JSON output depends on these values, so changing one is a contract change.
#[rustfmt::skip]
    const RECORDED_CODES: [(Code, &str, &str, &str); 44] = [
        (Code::GlobalConfigInvalid, "global_config_invalid", "error", "global_config"),
        (Code::TemplateInvalid, "template_invalid", "error", "template"),
        (Code::TemplateUnresolved, "template_unresolved", "warning", "template"),
        (Code::EnvironmentConfigInvalid, "environment_config_invalid", "error", "environment_config"),
        (Code::RunnerMissing, "runner_missing", "error", "wine"),
        (Code::BuildMissing, "build_missing", "error", "wine"),
        (Code::WinetricksMissing, "winetricks_missing", "warning", "wine"),
        (Code::WinetricksUnavailable, "winetricks_unavailable", "error", "wine"),
        (Code::PrefixMissing, "prefix_missing", "warning", "wine"),
        (Code::PrefixRefused, "prefix_refused", "error", "wine"),
        (Code::ExeMissing, "exe_missing", "error", "target"),
        (Code::CwdMissing, "cwd_missing", "error", "target"),
        (Code::GamemodeMissing, "gamemode_missing", "error", "target"),
        (Code::GamescopeMissing, "gamescope_missing", "error", "target"),
        (Code::MangohudMissing, "mangohud_missing", "error", "target"),
        (Code::WaylandUnsupported, "wayland_unsupported", "error", "target"),
        (Code::NativeExeInvalid, "native_exe_invalid", "error", "native_target"),
        (Code::TargetAmbiguous, "target_ambiguous", "warning", "environment"),
        (Code::LostRun, "lost_run", "warning", "runtime"),
        (Code::InstallerUnresolved, "installer_unresolved", "warning", "runtime"),
        (Code::RestoreInterrupted, "restore_interrupted", "error", "runtime"),
        (Code::StateDirStray, "state_dir_stray", "warning", "runtime"),
        (Code::DesktopEntryStray, "desktop_entry_stray", "warning", "desktop"),
        (Code::DesktopOutdated, "desktop_outdated", "warning", "desktop"),
        (Code::DesktopPortable, "desktop_portable", "warning", "desktop"),
        (Code::SteamNotFound, "steam_not_found", "error", "steam"),
        (Code::SteamInstallMissing, "steam_install_missing", "error", "steam"),
        (Code::SteamLibraryUnavailable, "steam_library_unavailable", "warning", "steam"),
        (Code::SteamDataUnknown, "steam_data_unknown", "error", "steam"),
        (Code::ArtConfigInvalid, "art_config_invalid", "error", "art"),
        (Code::ArtFileMissing, "art_file_missing", "warning", "art"),
        (Code::ArtTargetUnknown, "art_target_unknown", "warning", "art"),
        (Code::ArtDuplicates, "art_duplicates", "warning", "art"),
        (Code::ArtDirStray, "art_dir_stray", "warning", "art"),
        (Code::StatsStray, "stats_stray", "warning", "runtime"),
        (Code::TrashInterrupted, "trash_interrupted", "error", "trash"),
        (Code::TrashInvalid, "trash_invalid", "error", "trash"),
        (Code::GogStateInvalid, "gog_state_invalid", "error", "gog"),
        (Code::GogStateMissing, "gog_state_missing", "error", "gog"),
        (Code::GogFolderMissing, "gog_folder_missing", "warning", "gog"),
        (Code::GogFolderTaken, "gog_folder_taken", "error", "gog"),
        (Code::GogMidUpdate, "gog_mid_update", "error", "gog"),
        (Code::GogOwnerMissing, "gog_owner_missing", "warning", "gog"),
        (Code::DeprecatedKey, "deprecated_key", "warning", "config"),
    ];

#[test]
fn codes_match_the_recorded_table() {
    let listed = Code::ALL.map(|code| {
        (
            code,
            code.as_str(),
            code.level().as_str(),
            code.area().as_str(),
        )
    });
    assert_eq!(listed, RECORDED_CODES);
}

#[test]
fn codes_levels_and_areas_serialize_as_decision_six_shows() {
    for code in Code::ALL {
        let json = serde_json::to_value(code).unwrap();
        assert_eq!(json, code.as_str());
        assert_eq!(
            serde_json::to_value(code.area()).unwrap(),
            code.area().as_str()
        );
        assert_eq!(
            serde_json::to_value(code.level()).unwrap(),
            code.level().as_str()
        );
    }
    let f = Fixture::new();
    f.wine("zomboid", HEALTHY);
    let art = f.art_dir("zomboid");
    fs::create_dir_all(&art).unwrap();
    fs::write(
        art.join("art.toml"),
        "[icon]\nfile = \"icon.png\"\nmanual = \"a.png\"\n",
    )
    .unwrap();
    let problem = f.only(Code::ArtFileMissing);
    assert_eq!(
        serde_json::to_value(&problem).unwrap(),
        serde_json::json!({
            "environment": "zomboid",
            "area": "art",
            "level": "warning",
            "code": "art_file_missing",
            "message": "art: icon.png is recorded but missing",
            "fix": "Drop zomboid's missing icon record",
        })
    );
    let stray = Problem::stray(Code::ArtDirStray, "old", "gone".to_owned(), None);
    let json = serde_json::to_value(&stray).unwrap();
    assert_eq!(json["environment"], serde_json::Value::Null);
    assert_eq!(json["fix"], serde_json::Value::Null);
}

/// Every path under `dir` with its contents, symlinks unfollowed.
fn tree(dir: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                stack.push(path.clone());
            }
            let bytes = kind.is_file().then(|| fs::read(&path).unwrap());
            found.push((path, bytes));
        }
    }
    found.sort();
    found
}

#[test]
fn checking_changes_nothing_on_disk() {
    let f = Fixture::new();
    f.global("stop_timeout = 5\n");
    f.wine(
        "game",
        &format!("dxvk = 'latest'\n{HEALTHY}desktop = true\n"),
    );
    f.lost_run("game");
    f.unresolved_installer("game", Some(4242));
    fs::create_dir_all(f.paths.runtime_dir("oldgame")).unwrap();
    f.stray_entry("gone", "gone");
    let art = f.art_dir("game");
    fs::create_dir_all(&art).unwrap();
    fs::write(
        art.join("art.toml"),
        "[icon]\nfile = \"icon.png\"\nmanual = \"a\"\n",
    )
    .unwrap();

    let before = tree(&f.root);
    let report = f.check();
    assert!(report.problems.len() >= 6, "{:#?}", report.problems);
    check_environment(&f.paths, "game").unwrap();
    assert_eq!(tree(&f.root), before);
}

mod gog {
    use std::collections::HashMap;
    use std::os::unix::fs::MetadataExt;

    use super::*;
    use crate::gog::folder::{
        Committed, FolderLock, Journal, JournalKind, Platform, Target, Track,
    };

    const NEVER: &dyn Fn() -> bool = &|| false;
    const ID: u64 = 1094900565;

    fn state(folder: &Path, owner: Option<&str>, journal: bool) -> FolderState {
        let target = Target {
            gog_id: ID,
            platform: Platform::Linux,
            language: "en-US".to_owned(),
            track: Track::Installer,
            products: Vec::new(),
        };
        FolderState {
            device: Some(7),
            committed: Some(Committed {
                source: None,
                gog_id: ID,
                platform: target.platform,
                language: target.language.clone(),
                track: target.track,
                products: Vec::new(),
                files: Vec::new(),
            }),
            journal: journal.then(|| Journal {
                operation: "op".to_owned(),
                kind: JournalKind::Update,
                target,
                packages: Vec::new(),
                attempted: Vec::new(),
            }),
            ..FolderState::new(folder.to_owned(), owner.map(str::to_owned))
        }
    }

    fn save(f: &Fixture, state: &FolderState) {
        FolderLock::exclusive(&f.paths, &state.folder, NEVER)
            .unwrap()
            .write(state)
            .unwrap();
    }

    fn gog_report(f: &Fixture, devices: &[(&Path, u64)]) -> Report {
        let map: HashMap<PathBuf, u64> = devices
            .iter()
            .map(|(path, device)| (path.to_path_buf(), *device))
            .collect();
        let device_of = move |path: &Path| Ok(map.get(path).copied());
        let names = environment::names(&f.paths).unwrap();
        let mut report = Report::default();
        check_gog(
            &f.paths,
            None,
            &names,
            &BTreeSet::new(),
            &device_of,
            &mut report,
        );
        report
    }

    fn apply_fix(f: &Fixture, fix: &Fix) -> anyhow::Result<Vec<Note>> {
        apply(&f.paths, fix, |_, _| Ok(()))
    }

    #[test]
    fn a_missing_folder_is_dropped_only_when_its_drive_is_mounted() {
        let f = Fixture::new();
        let folder = Path::new("/mnt/games/rimworld");
        save(&f, &state(folder, None, false));

        // Unmounted: the mount point sits on the root filesystem.
        let report = gog_report(&f, &[(Path::new("/"), 1), (Path::new("/mnt/games"), 1)]);
        assert_eq!(codes(&report), [Code::GogFolderMissing]);
        assert_eq!(
            report.problems[0].message,
            "GOG game folder /mnt/games/rimworld is missing."
        );
        assert_eq!(report.problems[0].fix, None);

        // Remounted with the folder there: nothing to report.
        let report = gog_report(
            &f,
            &[
                (Path::new("/"), 1),
                (Path::new("/mnt/games"), 7),
                (folder, 7),
            ],
        );
        assert!(report.problems.is_empty(), "{:#?}", report.problems);

        // Mounted and really deleted: offered, and nothing else.
        let report = gog_report(&f, &[(Path::new("/"), 1), (Path::new("/mnt/games"), 7)]);
        assert_eq!(
            report.problems[0].fix,
            Some(Fix::ForgetGogFolder {
                folder: folder.to_owned()
            })
        );
    }

    #[test]
    fn forgetting_a_deleted_folder_rechecks_it() {
        let f = Fixture::new();
        let folder = f.root.join("Games/gone");
        fs::create_dir_all(&folder).unwrap();
        let mut gone = state(&folder, None, false);
        gone.device = Some(fs::metadata(&folder).unwrap().dev());
        fs::remove_dir(&folder).unwrap();
        save(&f, &gone);
        let fix = f.only(Code::GogFolderMissing).fix.unwrap();
        apply_fix(&f, &fix).unwrap();
        assert!(folder::list(&f.paths).unwrap().is_empty());
        assert!(apply_fix(&f, &fix).is_err());
    }

    #[test]
    fn mid_update_folders_name_their_owner() {
        let f = Fixture::new();
        f.native("rimworld", &format!("gog_id = '{ID}'\n"));
        let folder = f.root.join("rimworld");
        fs::create_dir_all(&folder).unwrap();
        save(&f, &state(&folder, Some("rimworld"), true));
        let problem = f.only(Code::GogMidUpdate);
        assert_eq!(problem.environment.as_deref(), Some("rimworld"));
        assert_eq!(
            problem.message,
            "rimworld's game folder is mid-update. Finish it with wineo update rimworld."
        );
        assert_eq!(problem.level, Level::Error);
        assert_eq!(problem.area, Area::Gog);
        let one = check_environment(&f.paths, "rimworld").unwrap();
        assert_eq!(codes(&one), [Code::GogMidUpdate]);
    }

    #[test]
    fn a_galaxy_install_names_the_install_command() {
        let f = Fixture::new();
        f.native("rimworld", &format!("gog_id = '{ID}'\n"));
        let folder = f.root.join("rimworld");
        fs::create_dir_all(&folder).unwrap();
        let mut galaxy = state(&folder, Some("rimworld"), true);
        let journal = galaxy.journal.as_mut().unwrap();
        journal.kind = JournalKind::Install;
        journal.target.track = Track::Galaxy;
        save(&f, &galaxy);
        assert_eq!(
            f.only(Code::GogMidUpdate).message,
            "rimworld's GOG install didn't finish. Finish it with wineo import gog --game 1094900565."
        );
    }

    #[test]
    fn a_lost_state_is_reported_with_how_to_fix_it() {
        let f = Fixture::new();
        f.native("rimworld", &format!("gog_id = '{ID}'\n"));
        let problem = f.only(Code::GogStateMissing);
        assert_eq!(
            problem.message,
            "rimworld's GOG state is missing. Migrate it again with wineo import gog --game 1094900565, or stop tracking it with wineo set rimworld --unset gog_id."
        );
        assert_eq!(
            codes(&check_environment(&f.paths, "rimworld").unwrap()),
            [Code::GogStateMissing]
        );
    }

    #[test]
    fn an_ownerless_folder_is_left_for_claiming_and_another_owner_is_reported() {
        let f = Fixture::new();
        f.native("rimworld", &format!("gog_id = '{ID}'\n"));
        f.native("copy", "");
        let folder = f.root.join("rimworld");
        fs::create_dir_all(&folder).unwrap();
        save(&f, &state(&folder, None, false));
        assert!(f.check().problems.is_empty(), "{:#?}", f.check().problems);

        save(&f, &state(&folder, Some("copy"), false));
        let problem = f.only(Code::GogFolderTaken);
        assert_eq!(
            problem.message,
            "rimworld's GOG game folder ~/rimworld belongs to copy."
        );
    }

    #[test]
    fn a_missing_owner_is_released_by_fix_keeping_any_journal() {
        for journal in [false, true] {
            let f = Fixture::new();
            let folder = f.root.join("rimworld");
            fs::create_dir_all(&folder).unwrap();
            save(&f, &state(&folder, Some("rimworld"), journal));
            let problem = f.only(Code::GogOwnerMissing);
            assert_eq!(
                problem.message,
                "GOG game folder ~/rimworld names rimworld, which doesn't exist."
            );
            let fix = problem.fix.unwrap();
            assert!(f.check().fixes().contains(&fix));
            apply_fix(&f, &fix).unwrap();
            let released = FolderLock::shared(&f.paths, &folder, NEVER)
                .unwrap()
                .read()
                .unwrap()
                .unwrap();
            assert_eq!(released.owner, None);
            assert_eq!(released.journal.is_some(), journal);
            assert!(!codes(&f.check()).contains(&Code::GogOwnerMissing));
        }
    }

    #[test]
    fn an_unreadable_state_is_reported() {
        let f = Fixture::new();
        let dir = f.paths.gog_folders_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("bad.toml"), "version = 9\nfolder = '/x'\n").unwrap();
        f.only(Code::GogStateInvalid);
    }

    #[test]
    fn a_portable_root_skips_host_gog_checks() {
        let f = Fixture::new();
        save(
            &f,
            &state(Path::new("/gone/rimworld"), Some("nobody"), true),
        );
        let portable = Fixture::xdg(&f.root, Some(&f.root.join("usb")));
        let report = check_all(&portable).unwrap();
        assert!(
            !report
                .problems
                .iter()
                .any(|problem| problem.area == Area::Gog),
            "{:#?}",
            report.problems
        );
    }

    #[test]
    fn folders_that_cant_be_looked_at_still_give_a_full_report() {
        use std::os::unix::fs::PermissionsExt;

        let f = Fixture::new();
        f.native("lost", "gog_id = '2'\n");
        // A parent component that is now a file.
        let file = f.root.join("was-a-dir");
        fs::write(&file, "").unwrap();
        let not_a_dir = file.join("game");
        save(&f, &state(&not_a_dir, None, false));
        // A parent that can't be searched.
        let locked = f.root.join("locked");
        fs::create_dir_all(locked.join("game")).unwrap();
        let denied = locked.join("game");
        save(&f, &state(&denied, None, false));
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let readable = fs::symlink_metadata(&denied).is_ok();

        let report = f.check();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        let unchecked: Vec<&Problem> = report
            .problems
            .iter()
            .filter(|problem| problem.code == Code::GogFolderMissing)
            .collect();
        assert!(
            unchecked
                .iter()
                .any(|problem| problem.message.contains("was-a-dir/game can't be checked")),
            "{:#?}",
            report.problems
        );
        // Running as root sees through the permissions.
        if !readable {
            assert!(
                unchecked
                    .iter()
                    .any(|problem| problem.message.contains("locked/game can't be checked")),
                "{:#?}",
                report.problems
            );
        }
        assert!(unchecked.iter().all(|problem| problem.fix.is_none()));
        assert!(codes(&report).contains(&Code::GogStateMissing));
    }

    #[test]
    fn validate_name_shows_an_unreadable_state_that_could_be_its_own() {
        let f = Fixture::new();
        f.native("rimworld", &format!("gog_id = '{ID}'\n"));
        f.native("plain", "");
        let dir = f.paths.gog_folders_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("bad.toml"), "version = 9\nfolder = '/x'\n").unwrap();
        let one = check_environment(&f.paths, "rimworld").unwrap();
        assert_eq!(codes(&one), [Code::GogStateInvalid, Code::GogStateMissing]);
        assert_eq!(
            one.problems[0].message.matches("bad.toml").count(),
            1,
            "{}",
            one.problems[0].message
        );
        let plain = check_environment(&f.paths, "plain").unwrap();
        assert!(plain.problems.is_empty(), "{:#?}", plain.problems);
    }

    #[test]
    fn validate_name_shows_an_unreadable_state_when_its_config_doesnt_parse() {
        let f = Fixture::new();
        f.write_config("broken", "kind = [");
        let dir = f.paths.gog_folders_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("bad.toml"), "version = 9\nfolder = '/x'\n").unwrap();
        let one = check_environment(&f.paths, "broken").unwrap();
        assert_eq!(
            codes(&one),
            [Code::EnvironmentConfigInvalid, Code::GogStateInvalid]
        );
    }
}

#[test]
fn deprecated_keys_are_reported_in_every_kind_of_config() {
    let f = Fixture::new();
    f.global("test_old_stop_timeout = 3\n");
    f.template("coop", "test_old_tags = ['game']\n");
    f.wine(
        "zomboid",
        "test_old_gamemode = false\n[targets.game]\ntest_old_exe = 'C:\\Game\\game.exe'\n",
    );
    let art = f.art_dir("zomboid");
    fs::create_dir_all(&art).unwrap();
    fs::write(art.join("art.toml"), "test_old_game = 5\n").unwrap();

    let report = f.check();
    let found: Vec<&Problem> = report
        .problems
        .iter()
        .filter(|problem| problem.code == Code::DeprecatedKey)
        .collect();
    assert_eq!(found.len(), 5, "{:#?}", report.problems);
    assert_eq!(report.errors(), 0, "{:#?}", report.problems);
    for problem in &found {
        assert_eq!(problem.level, Level::Warning);
        assert_eq!(problem.area, Area::Config);
    }
    let global = found
        .iter()
        .find(|problem| problem.message.contains("test_old_stop_timeout"))
        .unwrap();
    assert_eq!(global.environment, None);
    assert_eq!(
        global.message,
        "~/config/wineo/config.toml: `test_old_stop_timeout` is deprecated since wineo 0.0.0; use `stop_timeout`"
    );
    assert_eq!(
        global.fix,
        Some(Fix::RenameDeprecatedKeys(deprecated::Config::Global))
    );
    let template = found
        .iter()
        .find(|problem| problem.message.contains("test_old_tags"))
        .unwrap();
    assert_eq!(template.template.as_deref(), Some("coop"));
    let target = found
        .iter()
        .find(|problem| problem.message.contains("targets.game.test_old_exe"))
        .unwrap();
    assert_eq!(target.environment.as_deref(), Some("zomboid"));
    assert!(target.message.contains("use `targets.game.exe`"));
    let art = found
        .iter()
        .find(|problem| problem.message.contains("`test_old_game`"))
        .unwrap();
    assert_eq!(
        art.fix,
        Some(Fix::RenameDeprecatedKeys(deprecated::Config::Art(
            "zomboid".to_owned()
        )))
    );
    // One rename per file, however many keys it has.
    assert_eq!(
        report
            .fixes()
            .iter()
            .filter(|fix| matches!(fix, Fix::RenameDeprecatedKeys(_)))
            .count(),
        4
    );

    let report = check_environment(&f.paths, "zomboid").unwrap();
    assert_eq!(
        report
            .problems
            .iter()
            .filter(|problem| problem.code == Code::DeprecatedKey)
            .count(),
        3,
        "{:#?}",
        report.problems
    );

    for fix in f.check().fixes() {
        apply(&f.paths, &fix, no_download).unwrap();
    }
    let report = f.check();
    assert!(
        !codes(&report).contains(&Code::DeprecatedKey),
        "{:#?}",
        report.problems
    );
    assert_eq!(report.errors(), 0, "{:#?}", report.problems);
    assert_eq!(
        fs::read_to_string(f.paths.global_config()).unwrap(),
        "stop_timeout = 3\nschema_version = 1\n"
    );
    assert_eq!(
        fs::read_to_string(f.paths.templates_dir().join("coop.toml")).unwrap(),
        "tags = ['game']\nschema_version = 1\n"
    );
    assert_eq!(
        fs::read_to_string(f.art_dir("zomboid").join("art.toml")).unwrap(),
        "game = 5\nschema_version = 1\n"
    );
}

#[test]
fn both_spellings_are_the_files_invalid_config_error() {
    let f = Fixture::new();
    f.global("stop_timeout = 3\ntest_old_stop_timeout = 3\n");
    f.template("coop", "tags = []\ntest_old_tags = []\n");
    f.wine("zomboid", "gamemode = true\ntest_old_gamemode = true\n");
    let codes = f.codes();
    for code in [
        Code::GlobalConfigInvalid,
        Code::TemplateInvalid,
        Code::EnvironmentConfigInvalid,
    ] {
        assert!(codes.contains(&code), "{codes:?}");
    }
    assert!(!codes.contains(&Code::DeprecatedKey), "{codes:?}");
    let problem = f.only(Code::EnvironmentConfigInvalid);
    assert!(
        problem.message.contains("`test_old_gamemode`"),
        "{}",
        problem.message
    );
    assert!(
        problem.message.contains("`gamemode`"),
        "{}",
        problem.message
    );

    let f = Fixture::new();
    f.wine("zomboid", HEALTHY);
    let art = f.art_dir("zomboid");
    fs::create_dir_all(&art).unwrap();
    fs::write(art.join("art.toml"), "game = 5\ntest_old_game = 5\n").unwrap();
    assert!(f.codes().contains(&Code::ArtConfigInvalid));
}

#[test]
fn the_rename_fix_keeps_comments_and_order() {
    let f = Fixture::new();
    f.wine(
        "zomboid",
        "# launch settings\ntest_old_gamemode = true # on\nmangohud = false\n\n[targets.game]\n# the game\ntest_old_exe = 'C:\\Game\\game.exe'\nargs = ['-x']\n",
    );
    let path = f.paths.environment_config("zomboid");
    let before = fs::read_to_string(&path).unwrap();
    let problem = f
        .check()
        .problems
        .into_iter()
        .find(|problem| problem.code == Code::DeprecatedKey)
        .unwrap();
    apply(&f.paths, fix(&problem), no_download).unwrap();
    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(
        after,
        before
            .replace("test_old_gamemode = true", "gamemode = true")
            .replace("test_old_exe =", "exe =")
            .replace(
                "mangohud = false\n",
                "mangohud = false\nschema_version = 1\n"
            )
    );
    assert!(!f.codes().contains(&Code::DeprecatedKey));
    // Nothing left to rename, so a second run changes nothing.
    apply(&f.paths, fix(&problem), no_download).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), after);
}
