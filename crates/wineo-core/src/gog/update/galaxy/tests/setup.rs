//! Setup in the prefix, run through the fake `cmd`, which runs each batch
//! file's program line and writes the code a `.exit` file beside the
//! program names.

use super::*;
use crate::gog::update::UpdateError;
use crate::gog::update::galaxy::record::SetupStep;
use crate::gog::update::galaxy::setup;
use crate::gog::update::windows::SetupError;
use crate::gog::update::windows::tests::set_graphical;
use crate::operation::{Locks, Operation};

pub(super) const ISI_EXE: &str = "__redist\\ISI\\scriptinterpreter.exe";
pub(super) const VC_EXE: &str = "__redist\\MSVC2017\\VC_redist.x86.exe";
const FOLDER: &str = "SimCity™ 4 Délüxe & 100% (x86)";
const NESTED: &str = r#"/q:a /c:"msiexec /i vcredist.msi /qb! /l*v %temp%\vc.log" 100% done"#;

pub(super) fn set_before_setup_step(hook: impl FnMut(&Paths, usize) + 'static) {
    SETUP_STEPS.with(|count| count.set(0));
    BEFORE_SETUP_STEP.with(|cell| *cell.borrow_mut() = Some(Box::new(hook)));
}

/// A redistributable whose program sits in its depot with, when `code` is
/// given, the code the fake `cmd` ends it with.
fn dependency(id: &str, exe: &str, arguments: &str, code: Option<&str>) -> FakeDependency {
    let mut depot = FakeDepot::new(0).file(FakeFile::new(exe, format!("{id} program").as_bytes()));
    if let Some(code) = code {
        depot = depot.file(FakeFile::new(&format!("{exe}.exit"), code.as_bytes()));
    }
    FakeDependency::new(id, exe, arguments, depot)
}

/// GOG's script interpreter and one runtime, each ending with its code.
pub(super) fn with_setup(fx: &Fixture, isi: Option<&str>, vc: Option<&str>) {
    fx.content.set_dependencies(
        "1",
        &[
            dependency("ISI", ISI_EXE, "", isi),
            dependency("MSVC2017", VC_EXE, "/quiet /norestart", vc),
        ],
    );
}

/// The game with a script for ISI, listing the runtime.
pub(super) fn scripted(build: &str) -> FakeBuild {
    let mut game = Fixture::game(build)
        .depot(FakeDepot::new(GAME).file(FakeFile::new(
            &format!("goggame-{GAME}.script"),
            format!("script of {build}").as_bytes(),
        )))
        .dependency("MSVC2017");
    game.script_interpreter = true;
    game
}

/// One setup run the fake `cmd` saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Ran {
    program: String,
    dir: String,
    arguments: String,
}

impl Ran {
    /// The program's file name.
    pub(super) fn name(&self) -> &str {
        self.program.rsplit('\\').next().unwrap()
    }
}

impl Fixture {
    pub(super) fn ran(&self) -> Vec<Ran> {
        fs::read_to_string(&self.cmd_log)
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let mut parts = line.splitn(3, '\t');
                Ran {
                    program: parts.next().unwrap().to_owned(),
                    dir: parts.next().unwrap().to_owned(),
                    arguments: parts.next().unwrap_or_default().to_owned(),
                }
            })
            .collect()
    }

    pub(super) fn names(&self) -> Vec<String> {
        self.ran().iter().map(|ran| ran.name().to_owned()).collect()
    }

    pub(super) fn version_dir(&self, dependency: &str) -> PathBuf {
        let record = self.record().unwrap();
        let manifest = record
            .pending
            .as_ref()
            .map(|pending| &pending.manifest)
            .or(record
                .installed
                .as_ref()
                .map(|installed| &installed.manifest))
            .unwrap();
        let redist = manifest
            .setup
            .redist
            .iter()
            .find(|redist| redist.dependency == dependency)
            .unwrap();
        self.paths
            .gog_redist_dir()
            .join(dependency)
            .join(&redist.manifest)
    }

    /// Sets the code the fake `cmd` ends a stored program with.
    pub(super) fn exits(&self, dependency: &str, exe: &str, code: &str) {
        let path = self
            .version_dir(dependency)
            .join(format!("{}.exit", exe.replace('\\', "/")));
        fs::write(path, code).unwrap();
    }

    pub(super) fn launch_refused(&self) -> bool {
        folder::precheck(
            &self.paths,
            "game",
            Some(&GAME.to_string()),
            &[self.folder.join("bin/Game.exe")],
        )
        .is_err()
    }

    pub(super) fn prefix(&self) -> PathBuf {
        GlobalConfig::default()
            .prefix_root(&self.paths)
            .unwrap()
            .join("game")
    }
}

fn z(path: &Path) -> String {
    format!("Z:{}", path.display()).replace('/', "\\")
}

fn heroic(fx: &Fixture, product: u64, build: &str) -> String {
    format!(
        "/VERYSILENT /DIR=\"{}\" /Language=English /LANG=English /ProductId={product} \
         /galaxyclient /buildId={build} /versionName=\"v{build}\" /nodesktopshorctut \
         /nodesktopshortcut /lang-code=en-US /supportDir=\"{}\"",
        z(&fx.folder),
        z(&fx.support(product, "").components().collect::<PathBuf>()),
    )
}

fn setup_failure(error: &anyhow::Error) -> String {
    match error.downcast_ref::<GalaxyError>() {
        Some(GalaxyError::Setup { detail, .. }) => detail.clone(),
        _ => panic!("not a setup failure: {error:#}"),
    }
}

#[test]
fn progress_names_each_setup_step() {
    let fx = Fixture::new();
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, None);
    let plan = fx.plan();
    let mut seen = Vec::new();
    install::apply(&fx.paths, &mut fx.session(), &plan, &Token::new(), |step| {
        if let install::Progress::Setup { done, total, step } = step {
            seen.push((done, total, step));
        }
    })
    .unwrap();
    let step = |name: &str| Some(name.to_owned());
    assert_eq!(
        seen,
        [
            (0, 2, step("script interpreter")),
            (1, 2, step("MSVC2017")),
            (2, 2, None)
        ]
    );
}

#[test]
fn a_plan_lists_the_setup_steps_that_will_run() {
    let fx = Fixture::new();
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, None);
    let plan = fx.plan();
    assert_eq!(
        plan.setup_steps(&fx.paths),
        ["script interpreter", "MSVC2017"]
    );
    fx.apply(&plan).unwrap();

    // The new build changes ISI's step alone.
    fx.content.add_build(scripted("200"));
    fx.gog.lock().unwrap().newest_first = true;
    let update = fx.update_plan();
    assert_eq!(update.installed_version(), Some("v100"));
    assert_eq!(update.version_name(), Some("v200"));
    assert_eq!(update.setup_steps(&fx.paths), ["script interpreter"]);
    assert_eq!(update.moves_from(), None);

    // A reinstall runs them all again.
    use crate::gog::update::galaxy::update as galaxy_update;
    let galaxy_update::Planned::Update(reinstall) = galaxy_update::plan(
        &fx.paths,
        &mut fx.session(),
        "game",
        crate::gog::update::apply::Options {
            reinstall: true,
            ..Default::default()
        },
        &Token::new(),
    )
    .unwrap() else {
        panic!("a reinstall always runs");
    };
    assert_eq!(
        reinstall.setup_steps(&fx.paths),
        ["script interpreter", "MSVC2017"]
    );
}

#[test]
fn isi_runs_for_the_game_then_each_redistributable_with_its_arguments() {
    let mut fx = Fixture::new();
    // A game folder with what cmd would choke on written in a batch file
    // reaches ISI as it is.
    fx.folder = fx.folder.with_file_name(FOLDER);
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, None);
    let installed = fx.install();
    assert_installed(&fx);
    assert!(installed.needs_restart.is_empty());

    let ran = fx.ran();
    assert_eq!(fx.names(), ["scriptinterpreter.exe", "VC_redist.x86.exe"]);
    assert!(ran[0].program.ends_with(&ISI_EXE.replace("__redist", "")));
    assert_eq!(
        ran[0].dir,
        z(&fx.support(GAME, "").components().collect::<PathBuf>())
    );
    assert_eq!(ran[0].arguments, heroic(&fx, GAME, "100"));
    assert!(ran[0].arguments.contains(FOLDER), "{}", ran[0].arguments);
    let vc = fx.version_dir("MSVC2017");
    assert_eq!(
        ran[1].program,
        z(&vc.join("__redist/MSVC2017/VC_redist.x86.exe"))
    );
    assert_eq!(ran[1].dir, z(&vc.join("__redist/MSVC2017")));
    assert_eq!(ran[1].arguments, "/quiet /norestart");

    // Both recorded, with what identifies them.
    let record = fx.record().unwrap();
    let manifest = &record.installed.as_ref().unwrap().manifest;
    let script = manifest
        .files
        .iter()
        .find(|file| file.path == format!("goggame-{GAME}.script"))
        .unwrap();
    let redist = |id: &str| {
        manifest
            .setup
            .redist
            .iter()
            .find(|redist| redist.dependency == id)
            .unwrap()
            .manifest
            .clone()
    };
    assert_eq!(
        record.setup,
        [
            SetupStep::ScriptInterpreter {
                product: GAME,
                build: "100".to_owned(),
                script: script.md5.clone(),
                interpreter: redist("ISI"),
            },
            SetupStep::Redistributable {
                dependency: "MSVC2017".to_owned(),
                manifest: redist("MSVC2017"),
            },
        ]
    );

    // Each step left its batch file, with CRLF line endings, and a log in
    // the run directory.
    let runs = fx.paths.runtime_dir("game");
    for step in ["01", "02"] {
        let bat = runs.join(format!("gog-galaxy-setup-{step}.bat"));
        let text = fs::read_to_string(&bat).unwrap();
        assert!(text.starts_with("@echo off\r\ncd /d \""), "{text:?}");
        assert_eq!(text.matches("\r\n").count(), 4, "{text:?}");
        assert!(!text.replace("\r\n", "").contains('\n'), "{text:?}");
        assert!(runs.join(format!("gog-galaxy-setup-{step}.log")).is_file());
    }
    // The batch file names nothing itself; it all came from variables.
    let isi = fs::read_to_string(runs.join("gog-galaxy-setup-01.bat")).unwrap();
    assert_eq!(isi, setup::batch_file(false));
    // Which were set on those runs alone.
    let config = fs::read_to_string(fx.paths.environment_config("game")).unwrap();
    assert!(!config.contains("WINEO_SETUP"), "{config}");
}

#[test]
fn isi_runs_for_the_game_then_each_dlc_from_its_own_support_folder() {
    let fx = Fixture::new();
    fx.gog.lock().unwrap().dlc = vec![DLC];
    fx.content.add_build(
        scripted("100")
            .depot(FakeDepot::new(DLC).file(FakeFile::new("Data\\dlc.dat", b"dlc")))
            .product(GAME, "")
            .product(DLC, ""),
    );
    with_setup(&fx, None, None);
    fx.install();
    assert_installed(&fx);
    let ran = fx.ran();
    assert_eq!(
        fx.names(),
        [
            "scriptinterpreter.exe",
            "scriptinterpreter.exe",
            "VC_redist.x86.exe"
        ]
    );
    for (ran, product) in ran.iter().zip([GAME, DLC]) {
        let support = fx.support(product, "").components().collect::<PathBuf>();
        assert_eq!(ran.dir, z(&support));
        assert_eq!(ran.arguments, heroic(&fx, product, "100"));
    }
    let record = fx.record().unwrap();
    let products: Vec<u64> = record
        .setup
        .iter()
        .filter_map(|step| match step {
            SetupStep::ScriptInterpreter { product, .. } => Some(*product),
            _ => None,
        })
        .collect();
    assert_eq!(products, [GAME, DLC]);
}

#[test]
fn a_setup_program_gets_heroics_arguments_then_its_own() {
    let fx = Fixture::new();
    let mut game = Fixture::game("100")
        .depot(
            FakeDepot::new(GAME)
                .file(FakeFile::new("Setup\\setup.exe", b"setup program").flag("support")),
        )
        .product(GAME, "setup\\SETUP.EXE");
    game.products[0].temp_arguments = "/galaxyhelper %x".to_owned();
    fx.content.add_build(game);
    fx.install();
    assert_installed(&fx);
    let ran = fx.ran();
    assert_eq!(ran.len(), 1);
    let support = fx.support(GAME, "").components().collect::<PathBuf>();
    assert_eq!(ran[0].program, z(&support.join("Setup/setup.exe")));
    assert_eq!(ran[0].dir, z(&support));
    assert_eq!(
        ran[0].arguments,
        format!("{} /galaxyhelper %x", heroic(&fx, GAME, "100"))
    );
    let record = fx.record().unwrap();
    assert!(matches!(
        &record.setup[..],
        [SetupStep::Program { product: GAME, path, arguments, .. }]
            if path == "Setup/setup.exe" && *arguments == ran[0].arguments
    ));
}

#[test]
fn nested_quotes_an_msi_and_percent_signs_reach_the_program_verbatim() {
    let fx = Fixture::new();
    fx.content.add_build(
        Fixture::game("100")
            .dependency("MSVC2005")
            .dependency("XNA"),
    );
    fx.content.set_dependencies(
        "1",
        &[
            dependency(
                "MSVC2005",
                "__redist\\MSVC2005\\vcredist_x86.exe",
                NESTED,
                None,
            ),
            dependency("XNA", "__redist\\XNA\\xnafx40_redist.msi", "/quiet", None),
        ],
    );
    fx.install();
    let ran = fx.ran();
    assert_eq!(ran.len(), 2);
    assert_eq!(ran[0].arguments, NESTED);
    let xna = fx.version_dir("XNA").join("__redist/XNA");
    assert_eq!(ran[1].program, "C:\\windows\\system32\\msiexec.exe");
    assert_eq!(ran[1].dir, z(&xna));
    assert_eq!(
        ran[1].arguments,
        format!("/i \"{}\" /quiet", z(&xna.join("xnafx40_redist.msi")))
    );
}

#[test]
fn every_exit_code_class_is_read_from_the_code_file() {
    // (ISI's code, the runtime's code, the failure's detail, a restart)
    let cases: [(&str, &str, Option<&str>, bool); 9] = [
        ("0", "0", None, false),
        ("0", "3010", None, true),
        ("0", "1641", None, true),
        ("0", "1638", None, false),
        // Only redistributables have codes besides 0 that mean success.
        ("3010", "0", Some("exit code 3010"), false),
        ("1638", "0", Some("exit code 1638"), false),
        // The host would see 0 for 256 and 70000 % 256 = 112.
        ("0", "256", Some("exit code 256"), false),
        ("70000", "0", Some("exit code 70000"), false),
        ("0", "-1073741819", Some("exit code -1073741819"), false),
    ];
    for (isi, vc, failure, restart) in cases {
        let fx = Fixture::new();
        fx.content.add_build(scripted("100"));
        with_setup(&fx, Some(isi), Some(vc));
        let result = fx.apply(&fx.plan());
        match failure {
            None => {
                let installed = result.unwrap();
                assert_installed(&fx);
                let expected: &[&str] = if restart { &["MSVC2017"] } else { &[] };
                assert_eq!(installed.needs_restart, expected, "{isi} {vc}");
            }
            Some(detail) => {
                let error = result.unwrap_err();
                assert_eq!(setup_failure(&error), detail, "{isi} {vc}");
                assert!(fx.state().journal.is_some());
                assert!(fx.launch_refused());
            }
        }
    }
}

#[test]
fn no_code_file_and_a_missing_program_are_failures() {
    let fx = Fixture::new();
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, None);
    let plan = fx.plan();
    let version = plan
        .manifest_for_tests()
        .setup
        .redist
        .iter()
        .find(|redist| redist.dependency == "MSVC2017")
        .unwrap()
        .manifest
        .clone();
    set_before_setup_step(move |paths, step| {
        if step == 1 {
            // The runtime's program writes no code, as a crashed cmd would.
            let program = paths
                .gog_redist_dir()
                .join("MSVC2017")
                .join(&version)
                .join("__redist/MSVC2017/VC_redist.x86.exe.nocode");
            fs::write(program, "").unwrap();
        }
    });
    let error = fx.apply(&plan).unwrap_err();
    assert_eq!(setup_failure(&error), "no exit code");
    assert!(
        error
            .to_string()
            .starts_with("Setting up Fake Game stopped: MSVC2017 failed (no exit code)."),
        "{error}"
    );

    // A program that isn't there is cmd's 9009.
    fs::remove_file(
        fx.version_dir("MSVC2017")
            .join("__redist/MSVC2017/VC_redist.x86.exe.nocode"),
    )
    .unwrap();
    fs::remove_file(
        fx.version_dir("MSVC2017")
            .join("__redist/MSVC2017/VC_redist.x86.exe"),
    )
    .unwrap();
    let error = fx.apply(&fx.plan()).unwrap_err();
    assert_eq!(
        setup_failure(&error),
        "exit code 9009: its program wasn't found"
    );
}

#[test]
fn a_failed_step_blocks_launch_and_installing_again_carries_on_from_it() {
    let fx = Fixture::new();
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, Some("5"));
    let error = fx.apply(&fx.plan()).unwrap_err();
    assert_eq!(setup_failure(&error), "exit code 5");
    assert!(fx.state().journal.is_some());
    assert!(fx.state().committed.is_none());
    assert!(fx.launch_refused());
    // ISI finished and is recorded; the runtime isn't.
    let record = fx.record().unwrap();
    assert!(matches!(
        &record.setup[..],
        [SetupStep::ScriptInterpreter { .. }]
    ));
    assert_eq!(fx.names(), ["scriptinterpreter.exe", "VC_redist.x86.exe"]);

    // Fixed, installing again downloads nothing and runs only the runtime.
    fx.exits("MSVC2017", VC_EXE, "0");
    let chunks = fx.chunk_requests().len();
    let plan = fx.plan();
    assert!(plan.resumes());
    fx.apply(&plan).unwrap();
    assert_installed(&fx);
    assert_eq!(fx.chunk_requests().len(), chunks);
    assert_eq!(
        fx.names(),
        [
            "scriptinterpreter.exe",
            "VC_redist.x86.exe",
            "VC_redist.x86.exe"
        ]
    );
    assert!(!fx.launch_refused());
    assert_eq!(fx.record().unwrap().setup.len(), 2);
}

#[test]
fn a_crash_between_a_step_and_its_record_runs_it_again() {
    let fx = Fixture::new();
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, None);
    // ISI exits 0, then the crash comes before it's recorded.
    interrupted(&fx, "galaxy.setup.ran", 0);
    assert!(fx.record().unwrap().setup.is_empty());
    assert!(fx.launch_refused());
    // Once more, after both ran and before the commit.
    let plan = fx.plan();
    set_failpoint(Some("galaxy.setup.after"));
    fx.apply(&plan).unwrap_err();
    set_failpoint(None);
    assert_eq!(fx.record().unwrap().setup.len(), 2);
    fx.apply(&fx.plan()).unwrap();
    assert_installed(&fx);
    assert_eq!(
        fx.names(),
        [
            "scriptinterpreter.exe",
            "scriptinterpreter.exe",
            "VC_redist.x86.exe"
        ]
    );
}

#[test]
fn a_changed_version_script_or_interpreter_reruns_only_its_step() {
    let fx = Fixture::new();
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, None);
    fx.install();
    let record = fx.record().unwrap();
    let installed = record.installed.clone().unwrap().manifest;
    let state = fx.state();
    let target = folder::Target {
        gog_id: GAME,
        platform: Platform::Windows,
        language: state.committed.as_ref().unwrap().language.clone(),
        track: Track::Galaxy,
        products: state.committed.as_ref().unwrap().products.clone(),
    };
    let support = fx.support(GAME, "");
    let label = |_: u64| "Fake Game".to_owned();
    let to_run = |manifest: &record::Manifest| {
        let (steps, _) = setup::steps(&setup::Sources {
            paths: &fx.paths,
            folder: &fx.folder,
            support: &support,
            manifest,
            target: &target,
            label: &label,
            review: false,
        })
        .unwrap();
        let wanted: Vec<SetupStep> = steps.into_iter().map(|step| step.identity).collect();
        setup::remaining(&wanted, &record.setup, false)
            .into_iter()
            .map(|index| match &wanted[index] {
                SetupStep::ScriptInterpreter { .. } => "ISI".to_owned(),
                SetupStep::Program { .. } => "program".to_owned(),
                SetupStep::Redistributable { dependency, .. } => dependency.clone(),
            })
            .collect::<Vec<_>>()
    };
    assert!(to_run(&installed).is_empty());
    let changed = |change: &dyn Fn(&mut record::Manifest)| {
        let mut manifest = installed.clone();
        change(&mut manifest);
        to_run(&manifest)
    };
    let redist = |manifest: &mut record::Manifest, id: &str| {
        let at = manifest
            .setup
            .redist
            .iter()
            .position(|redist| redist.dependency == id)
            .unwrap();
        manifest.setup.redist[at].manifest = "e".repeat(32);
    };
    assert_eq!(
        changed(&|manifest| redist(manifest, "MSVC2017")),
        ["MSVC2017"]
    );
    assert_eq!(changed(&|manifest| redist(manifest, "ISI")), ["ISI"]);
    let script = format!("goggame-{GAME}.script");
    assert_eq!(
        changed(&|manifest| {
            let file = manifest
                .files
                .iter_mut()
                .find(|file| file.path == script)
                .unwrap();
            file.md5 = Some("f".repeat(32));
        }),
        ["ISI"]
    );
    // A new build reruns ISI, and --reinstall or --repair everything.
    assert_eq!(
        changed(&|manifest| manifest.build = "200".to_owned()),
        ["ISI"]
    );
    let (steps, _) = setup::steps(&setup::Sources {
        paths: &fx.paths,
        folder: &fx.folder,
        support: &support,
        manifest: &installed,
        target: &target,
        label: &label,
        review: false,
    })
    .unwrap();
    let wanted: Vec<SetupStep> = steps.into_iter().map(|step| step.identity).collect();
    assert_eq!(setup::remaining(&wanted, &record.setup, true), [0, 1]);
}

#[test]
fn two_games_set_up_two_versions_of_one_redistributable_at_once() {
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
    // Both wait before their setup, holding their versions, until both are
    // there.
    let barrier = Arc::new(std::sync::Barrier::new(2));
    std::thread::scope(|scope| {
        for plan in [&first, &second] {
            let (fx, barrier) = (&fx, Arc::clone(&barrier));
            scope.spawn(move || {
                set_before_setup_step(move |_, _| {
                    barrier.wait();
                });
                fx.apply(plan).unwrap();
            });
        }
    });
    let dir = fx.paths.gog_redist_dir().join("MSVC2017");
    let mut programs: Vec<String> = fx.ran().into_iter().map(|ran| ran.program).collect();
    programs.sort();
    let mut expected: Vec<String> = [&first, &second]
        .iter()
        .map(|plan| {
            z(&dir
                .join(&plan.manifest_for_tests().setup.redist[0].manifest)
                .join("__redist/MSVC2017/VC_redist.x86.exe"))
        })
        .collect();
    expected.sort();
    assert_ne!(expected[0], expected[1]);
    assert_eq!(programs, expected);
}

#[test]
fn cleanup_between_a_download_and_setup_keeps_the_version_setup_runs() {
    let fx = Fixture::new();
    fx.content
        .add_build(Fixture::game("100").dependency("MSVC2017"));
    with_dependencies(&fx, b"runtime v1");
    let plan = fx.plan();
    let version = plan.manifest_for_tests().setup.redist[0].manifest.clone();
    let kept = fx.paths.gog_redist_dir().join("MSVC2017").join(&version);
    // Another game's install finishing and cleaning up, right before this
    // one's setup.
    set_before_setup_step(|paths, _| {
        assert!(store::clean(paths).unwrap().is_empty());
    });
    fx.apply(&plan).unwrap();
    assert!(kept.is_dir());
    assert_eq!(
        fx.ran()[0].program,
        z(&kept.join("__redist/MSVC2017/VC_redist.x86.exe"))
    );
}

#[test]
fn a_snapshot_restore_during_setup_is_refused() {
    let fx = Fixture::new();
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, None);
    let prefix = fx.prefix();
    let refused = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = std::rc::Rc::clone(&refused);
    set_before_setup_step(move |paths, step| {
        let busy = Locks::new(paths)
            .acquire(Operation::Snapshot, "game", &[&prefix])
            .is_err();
        seen.borrow_mut().push((step, busy));
    });
    fx.install();
    assert_eq!(*refused.borrow(), [(0, true), (1, true)]);
    assert!(
        Locks::new(&fx.paths)
            .acquire(Operation::Snapshot, "game", &[&fx.prefix()])
            .is_ok()
    );
}

#[test]
fn canceling_during_setup_stops_and_leaves_the_journal() {
    let fx = Fixture::new();
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, None);
    let token = Token::new();
    let cancel = token.clone();
    set_before_setup_step(move |_, step| {
        if step == 1 {
            cancel.cancel();
        }
    });
    let error =
        install::apply(&fx.paths, &mut fx.session(), &fx.plan(), &token, |_| {}).unwrap_err();
    assert!(
        matches!(
            error.downcast_ref::<UpdateError>(),
            Some(UpdateError::Canceled { journal: true })
        ),
        "{error:#}"
    );
    // ISI ran; the runtime never started.
    assert_eq!(fx.names(), ["scriptinterpreter.exe"]);
    assert!(fx.state().journal.is_some());
    set_before_setup_step(|_, _| {});
    fx.apply(&fx.plan()).unwrap();
    assert_installed(&fx);
    assert_eq!(fx.names(), ["scriptinterpreter.exe", "VC_redist.x86.exe"]);
}

#[test]
fn a_galaxy_install_needs_a_graphical_session() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    let plan = fx.plan();
    set_graphical(false);
    let refused = |error: anyhow::Error| {
        matches!(
            error.downcast_ref::<SetupError>(),
            Some(SetupError::NoGraphicalSession)
        )
    };
    assert!(refused(fx.plan_with(&fx.request()).unwrap_err()));
    assert!(refused(fx.apply(&plan).unwrap_err()));
    assert!(!fx.folder.exists());
    set_graphical(true);
    fx.apply(&plan).unwrap();
    assert_installed(&fx);
}

#[test]
fn steps_a_leftover_record_shows_done_still_run_for_a_new_install() {
    let fx = Fixture::new();
    fx.content
        .add_build(Fixture::game("100").dependency("MSVC2017"));
    with_dependencies(&fx, b"runtime v1");
    let plan = fx.plan();
    // A record with no state beside it, naming this very step done.
    let redist = &plan.manifest_for_tests().setup.redist[0];
    let leftover = Record {
        setup: vec![SetupStep::Redistributable {
            dependency: redist.dependency.clone(),
            manifest: redist.manifest.clone(),
        }],
        ..Record::default()
    };
    write_record(&fx, &leftover);
    fx.apply(&plan).unwrap();
    assert_eq!(fx.names(), ["VC_redist.x86.exe"]);
    assert_eq!(fx.record().unwrap().setup, leftover.setup);
}

#[test]
fn a_run_directory_cmd_cant_be_given_is_refused_clearly() {
    let mut fx = Fixture::new();
    let home = fx._root.path().to_owned();
    let state = home.join("Zoë's state");
    fx.paths = Paths::select(
        move |key| match key {
            "HOME" => Some(home.clone().into_os_string()),
            "XDG_STATE_HOME" => Some(state.clone().into_os_string()),
            _ => None,
        },
        None,
    )
    .unwrap();
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, None);
    let error = fx.apply(&fx.plan()).unwrap_err();
    assert!(
        error.to_string().starts_with("Setup can't run from "),
        "{error:#}"
    );
    assert!(fx.ran().is_empty());
    assert!(fx.state().journal.is_some());
}

/// Set for the copy of the test binary that runs with a decoy `cmd` in its
/// current folder.
const DECOY: &str = "WINEO_TEST_DECOY_CMD";

#[test]
fn a_file_named_cmd_in_the_current_folder_is_never_run() {
    let dir = tempfile::tempdir().unwrap();
    crate::test_script::write_script(
        dir.path().join("cmd"),
        "#!/bin/sh\ntouch \"$0.ran\"\nexit 0\n",
    );
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "gog::update::galaxy::tests::setup::decoy_cmd_scenario",
            "--nocapture",
        ])
        .env(DECOY, "1")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "{output:?}"
    );
    assert!(!dir.path().join("cmd.ran").exists());
}

/// Runs only as `a_file_named_cmd_in_the_current_folder_is_never_run`'s
/// copy, whose current folder holds a file named `cmd`.
#[test]
fn decoy_cmd_scenario() {
    if std::env::var_os(DECOY).is_none() {
        return;
    }
    assert!(Path::new("cmd").is_file());
    let fx = Fixture::new();
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, None);
    fx.install();
    assert_installed(&fx);
    assert_eq!(fx.names(), ["scriptinterpreter.exe", "VC_redist.x86.exe"]);
}

#[test]
fn a_depot_language_with_cmd_syntax_never_reaches_cmd() {
    let fx = Fixture::new();
    let injected = "en-US&echo injected>injected.txt&rem";
    let mut game = scripted("100");
    for depot in &mut game.depots {
        depot.languages = vec![injected.to_owned()];
    }
    fx.content.add_build(game);
    with_setup(&fx, None, None);
    // Taken as the game's language, it's refused before anything runs.
    let plan = fx.plan();
    assert_eq!(plan.language(), injected);
    let error = fx.apply(&plan).unwrap_err();
    assert!(format!("{error:#}").contains("language code"), "{error:#}");
    assert!(fx.ran().is_empty());
    assert!(fx.state().journal.is_some());
}
