//! Repairs, reinstalls, and moves between tracks against the fake content
//! system.

use super::setup::{VC_EXE, scripted, with_setup};
use super::update::{
    assert_updated, chunk, forget_record, game_200, in_english, update_interrupted,
};
use super::*;
use crate::gog::update::UpdateError;
use crate::gog::update::apply::{Options, Updated};
use crate::gog::update::galaxy::update::{self, Note, Planned, Start};
use crate::gog::update::galaxy::{PlannedUpdate, Progress, plan_update};

const CORE: &str = "core data, several chunks long";

fn repair() -> Options {
    Options {
        repair: true,
        refresh: true,
        ..Options::default()
    }
}

fn reinstall() -> Options {
    Options {
        reinstall: true,
        refresh: true,
        ..Options::default()
    }
}

fn to_galaxy() -> Options {
    Options {
        track: Some(Track::Galaxy),
        ..reinstall()
    }
}

impl Fixture {
    fn op(&self, options: Options) -> Result<update::Plan> {
        match update::plan(
            &self.paths,
            &mut self.session(),
            "game",
            options,
            &Token::new(),
        )? {
            Planned::Update(plan) => Ok(plan),
            Planned::UpToDate { .. } => panic!("up to date"),
        }
    }

    fn run(&self, plan: &update::Plan) -> (Updated, Vec<Progress>) {
        let mut steps = Vec::new();
        let updated = update::apply(
            &self.paths,
            &mut self.session(),
            plan,
            &Token::new(),
            |step| steps.push(step),
        )
        .unwrap();
        (updated, steps)
    }

    fn repaired(&self) -> (Updated, Vec<Progress>) {
        self.run(&self.op(repair()).unwrap())
    }

    fn builds_requests(&self) -> usize {
        self.server
            .requests()
            .iter()
            .filter(|request| request.path().ends_with("/os/windows/builds"))
            .count()
    }

    fn damage(&self, path: &str, content: &str) {
        fs::write(self.folder.join(path), content).unwrap();
    }

    fn saved(updated: &Updated) -> Vec<&str> {
        let mut saved: Vec<&str> = updated.saved.iter().map(|s| s.path.as_str()).collect();
        saved.sort_unstable();
        saved
    }
}

/// Build 100 as `Fixture::game` has it, committed with a trusted record, and
/// every file as GOG ships it.
fn assert_whole(fx: &Fixture, build: &str) {
    let state = fx.state();
    assert_eq!(state.journal, None);
    assert_eq!(fx.read("bin/Game.exe"), "MZ the game itself");
    assert_eq!(fx.read("Data/core.dat"), CORE);
    assert_eq!(fx.read("Data/big.dat"), "many chunks and no whole-file md5");
    assert_eq!(fx.read("empty.txt"), "");
    assert!(fx.folder.join("saves").is_dir());
    assert_eq!(
        fs::read_link(fx.folder.join("Data/current")).unwrap(),
        Path::new("core.dat")
    );
    assert_eq!(fx.read(&format!("goggame-{GAME}.info")), info(GAME, build));
    assert_eq!(fx.base_build().as_deref(), Some(build));
    let record = fx.record().unwrap();
    assert!(record::pending(&state, &record).is_none());
    assert!(record::installed(&state, &record).unwrap().is_some());
    assert!(!fx.launch_refused());
}

#[test]
fn repair_fixes_same_size_damage_that_an_updates_size_check_keeps() {
    let fx = Fixture::new();
    // Build 200 only changes the info file.
    fx.installed_then(Fixture::game("200"));
    forget_record(&fx);
    let damaged = "core data, several chunks LONG";
    fx.damage("Data/core.dat", damaged);
    let plan = fx.update_plan();
    assert_eq!(plan.start(), Start::Fetched);
    fx.apply_update(&plan).unwrap();
    // The size check can't see it.
    assert_eq!(fx.read("Data/core.dat"), damaged);

    let plan = fx.op(repair()).unwrap();
    assert_eq!(plan.kind(), JournalKind::Repair);
    assert_eq!(plan.start(), Start::Stored);
    assert!(plan.notes().is_empty());
    let before = fx.chunk_count();
    let (updated, steps) = fx.run(&plan);
    assert_eq!(updated.kind, JournalKind::Repair);
    assert!(!updated.recovered);
    assert_whole(&fx, "200");
    // Only the chunk the damage touched; the rest of the damaged copy is
    // read back.
    assert_eq!(fx.chunks_since(before), [chunk(b"s long")]);
    // Yours is kept aside and listed, never overwritten.
    assert_eq!(fx.read("Data/core.dat.wineo-saved"), damaged);
    assert_eq!(Fixture::saved(&updated), ["Data/core.dat"]);
    assert!(updated.overwritten.is_empty() && updated.kept.is_empty());
    // Hashing is reported to the end.
    let last = steps
        .iter()
        .rev()
        .find_map(|step| match step {
            Progress::Verify {
                files,
                files_total,
                bytes,
                bytes_total,
            } => Some((*files, *files_total, *bytes, *bytes_total)),
            _ => None,
        })
        .expect("a hashing step");
    assert_eq!(last.0, last.1);
    assert_eq!(last.2, last.3);
    assert!(last.1 >= 4);
}

#[test]
fn repair_restores_missing_files_folders_and_links_and_saves_an_edit() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    fx.install();
    fs::remove_file(fx.folder.join("bin/Game.exe")).unwrap();
    fs::remove_dir(fx.folder.join("saves")).unwrap();
    fs::remove_file(fx.folder.join("Data/current")).unwrap();
    fx.damage("Data/big.dat", "mine");
    // Left alone: not in the manifest.
    fx.damage("notes.txt", "yours");
    let (updated, _) = fx.repaired();
    assert_whole(&fx, "100");
    assert_eq!(Fixture::saved(&updated), ["Data/big.dat"]);
    assert_eq!(fx.read("Data/big.dat.wineo-saved"), "mine");
    assert_eq!(fx.read("notes.txt"), "yours");
}

/// The game with a support file and a game-local dependency.
fn with_support(build: &str) -> FakeBuild {
    Fixture::game(build)
        .depot(
            FakeDepot::new(GAME)
                .file(FakeFile::new("__support\\setup.exe", b"setup program").flag("support")),
        )
        .dependency("DOSBOX")
}

fn dosbox(fx: &Fixture) {
    fx.content.set_dependencies(
        "1",
        &[FakeDependency::new(
            "DOSBOX",
            "",
            "",
            FakeDepot::new(0).file(FakeFile::new("DOSBOX\\dosbox.exe", b"dosbox")),
        )],
    );
}

#[test]
fn repair_restores_support_files_and_game_local_dependency_files() {
    let fx = Fixture::new();
    dosbox(&fx);
    fx.content.add_build(with_support("100"));
    fx.install();
    let support = fx.support(GAME, "__support/setup.exe");
    assert!(support.is_file());
    // Missing, and same-size damage.
    fs::remove_file(&support).unwrap();
    fx.damage("DOSBOX/dosbox.exe", "dosboX");
    let (updated, _) = fx.repaired();
    assert_eq!(fs::read_to_string(&support).unwrap(), "setup program");
    assert_eq!(fx.read("DOSBOX/dosbox.exe"), "dosbox");
    assert_eq!(fx.read("DOSBOX/dosbox.exe.wineo-saved"), "dosboX");
    assert_eq!(Fixture::saved(&updated), ["DOSBOX/dosbox.exe"]);

    // Support files are wineo's own: damage there is replaced, not saved.
    fs::write(&support, "setup prograx").unwrap();
    let (updated, _) = fx.repaired();
    assert_eq!(fs::read_to_string(&support).unwrap(), "setup program");
    assert!(updated.saved.is_empty());
}

#[test]
fn repair_reruns_every_setup_step_and_a_clean_one_still_journals() {
    let fx = Fixture::new();
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, None);
    fx.install();
    assert_eq!(fx.names(), ["scriptinterpreter.exe", "VC_redist.x86.exe"]);
    let state = fx.state();
    let record = fx.record().unwrap();
    // The record matches, so an update would carry every step over.
    assert!(record::installed(&state, &record).unwrap().is_some());
    assert_eq!(record.setup.len(), 2);

    let before = fx.chunk_count();
    let plan = fx.op(repair()).unwrap();
    assert_eq!(plan.start(), Start::Stored);
    let (updated, _) = fx.run(&plan);
    assert_eq!(updated.kind, JournalKind::Repair);
    assert_eq!(
        fx.names(),
        [
            "scriptinterpreter.exe",
            "VC_redist.x86.exe",
            "scriptinterpreter.exe",
            "VC_redist.x86.exe"
        ]
    );
    // Nothing was damaged.
    assert_eq!(fx.chunk_count(), before);
    assert!(updated.saved.is_empty());
    let record = fx.record().unwrap();
    assert_eq!(record.setup.len(), 2);
    assert!(record::installed(&fx.state(), &record).unwrap().is_some());
    assert!(!fx.launch_refused());
    // A failed step is retried without hashing again on finishing.
    let _ = VC_EXE;
}

#[test]
fn repair_with_no_trusted_record_fetches_the_build_and_deletes_nothing() {
    let fx = Fixture::new();
    fx.installed_then(game_200());
    forget_record(&fx);
    fx.damage("Data/core.dat", "shorter");
    fx.damage("extra.txt", "yours");
    let plan = fx.op(repair()).unwrap();
    // The installed build, not the newer one listed first.
    assert_eq!(plan.build(), "100");
    assert_eq!(plan.start(), Start::Target);
    assert_eq!(plan.notes(), [Note::NothingDeleted]);
    let before = fx.chunk_count();
    let (updated, _) = fx.run(&plan);
    assert_whole(&fx, "100");
    assert_eq!(fx.read("Data/core.dat.wineo-saved"), "shorter");
    assert_eq!(Fixture::saved(&updated), ["Data/core.dat"]);
    assert_eq!(fx.read("extra.txt"), "yours");
    // What matched stayed.
    assert!(!fx.chunks_since(before).contains(&chunk(b"MZ the g")));
}

#[test]
fn repair_needs_no_listing_with_a_trusted_record_and_refuses_without_one() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    fx.install();
    fx.hide("100");
    fx.damage("Data/core.dat", "core data, several chunks LONG");
    let asked = fx.builds_requests();
    let plan = fx.op(repair()).unwrap();
    assert_eq!(plan.start(), Start::Stored);
    fx.run(&plan);
    assert_eq!(fx.builds_requests(), asked, "no build list was needed");
    assert_whole(&fx, "100");

    // Without the record the build has to come from GOG.
    forget_record(&fx);
    fx.damage("Data/core.dat", "core data, several chunks LONG");
    let before = fx.state();
    let error = fx.op(repair()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "GOG no longer lists build 100 of Fake Game. Run wineo update game to move to the latest."
    );
    assert!(matches!(
        error.downcast_ref(),
        Some(GalaxyError::BuildGone { .. })
    ));
    assert_eq!(fx.state(), before);
    assert_eq!(fx.read("Data/core.dat"), "core data, several chunks LONG");
}

#[test]
fn an_unstarted_repair_never_makes_a_file_tracked() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    fx.install();
    forget_record(&fx);
    fx.damage("Data/core.dat", "yours");
    // Stopped after pending, before the journal.
    set_failpoint(Some("galaxy.update.pending.after"));
    let error = fx.run_err(&fx.op(repair()).unwrap());
    set_failpoint(None);
    assert!(format!("{error:#}").contains("injected"), "{error:#}");
    assert!(fx.state().journal.is_none());
    // Its pending claims the file of yours is the build's.
    let mut record = fx.record().unwrap();
    assert!(record.installed.is_none());
    let pending = record.pending.as_mut().unwrap();
    for file in &mut pending.manifest.files {
        if file.path == "Data/core.dat" {
            file.md5 = Some(fake::content::md5_hex(b"yours"));
        }
    }
    write_record(&fx, &record);
    let plan = fx.op(repair()).unwrap();
    assert!(!plan.is_recovery());
    assert_eq!(plan.start(), Start::Target);
    let (updated, _) = fx.run(&plan);
    assert_eq!(fx.read("Data/core.dat.wineo-saved"), "yours");
    assert_eq!(Fixture::saved(&updated), ["Data/core.dat"]);
    assert_whole(&fx, "100");
}

impl Fixture {
    fn run_err(&self, plan: &update::Plan) -> anyhow::Error {
        update::apply(
            &self.paths,
            &mut self.session(),
            plan,
            &Token::new(),
            |_| {},
        )
        .unwrap_err()
    }
}

#[test]
fn an_interrupted_repair_is_finished_as_a_repair() {
    for point in [
        "galaxy.update.pending.after",
        "galaxy.update.journal.after",
        "galaxy.update.placed.after",
        "galaxy.update.files.after",
        "galaxy.update.setup.after",
        "galaxy.update.installed.after",
        "galaxy.update.commit.after",
    ] {
        let fx = Fixture::new();
        fx.content.add_build(scripted("100"));
        with_setup(&fx, None, None);
        fx.install();
        fx.damage("Data/core.dat", "core data, several chunks LONG");
        fs::remove_file(fx.folder.join("bin/Game.exe")).unwrap();
        set_failpoint(Some(point));
        let error = fx.run_err(&fx.op(repair()).unwrap());
        set_failpoint(None);
        assert!(format!("{error:#}").contains("injected"), "{point}");
        let journaled = fx.state().journal.is_some();
        assert_eq!(
            journaled,
            !point.ends_with("pending.after") && !point.ends_with("commit.after")
        );
        if journaled {
            // The journal wins over the options.
            let plan = fx
                .op(Options {
                    refresh: true,
                    ..Options::default()
                })
                .unwrap();
            assert_eq!(plan.kind(), JournalKind::Repair, "{point}");
            assert!(plan.is_recovery(), "{point}");
            let (updated, _) = fx.run(&plan);
            assert_eq!(updated.kind, JournalKind::Repair);
            assert!(updated.recovered);
        } else if point.ends_with("pending.after") {
            fx.repaired();
        }
        assert_whole(&fx, "100");
        assert_eq!(
            fx.read("Data/core.dat.wineo-saved"),
            "core data, several chunks LONG",
            "{point}"
        );
        // Not run twice in a row: each setup step ran for the install, and
        // again for the repair (and no more than once more on a resume that
        // had got past it).
        let names = fx.names();
        assert!(names.len() >= 4 && names.len() <= 6, "{point}: {names:?}");
    }
}

#[test]
fn repair_is_refused_for_the_installer_track_and_native_games_and_bad_combinations() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    fx.install();
    as_installer_game(&fx);
    let error = plan_update(
        &fx.paths,
        Some(&mut fx.session()),
        "game",
        repair(),
        &Token::new(),
    )
    .unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(UpdateError::NotGalaxy { .. })
    ));
    assert_eq!(
        error.to_string(),
        "Only Galaxy games have branches. Move it with --reinstall --track galaxy."
    );
    // Native: Galaxy has no Linux builds.
    let lock = FolderLock::exclusive(&fx.paths, &fx.folder, &|| false).unwrap();
    let mut state = lock.read().unwrap().unwrap();
    state.committed.as_mut().unwrap().platform = Platform::Linux;
    lock.write(&state).unwrap();
    drop(lock);
    for options in [repair(), to_galaxy()] {
        let error = plan_update(
            &fx.paths,
            Some(&mut fx.session()),
            "game",
            options,
            &Token::new(),
        )
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref(),
            Some(GalaxyError::NotWindows)
        ));
    }
    // Combinations that mean nothing.
    for options in [
        Options {
            repair: true,
            reinstall: true,
            ..Options::default()
        },
        Options {
            repair: true,
            track: Some(Track::Galaxy),
            ..Options::default()
        },
        Options {
            track: Some(Track::Installer),
            ..Options::default()
        },
    ] {
        let error = plan_update(
            &fx.paths,
            Some(&mut fx.session()),
            "game",
            options,
            &Token::new(),
        )
        .unwrap_err();
        assert!(
            matches!(error.downcast_ref(), Some(UpdateError::Options { .. })),
            "{error:#}"
        );
        assert!(
            update::plan(&fx.paths, &mut fx.session(), "game", options, &Token::new()).is_err()
        );
        assert!(
            crate::gog::update::plan(
                &fx.paths,
                Some(&mut fx.session()),
                "game",
                options,
                &Token::new()
            )
            .is_err()
        );
    }
}

// Reinstalling on Galaxy.

#[test]
fn a_reinstall_across_a_newer_build_hashes_everything() {
    let fx = Fixture::new();
    // Build 200 changes the info file and others; core.dat stays.
    fx.installed_then(game_200());
    let damaged = "core data, several chunks LONG";
    fx.damage("Data/core.dat", damaged);
    fs::write(fx.folder.join("empty.txt"), "").unwrap();
    let plan = fx.op(reinstall()).unwrap();
    assert_eq!(plan.kind(), JournalKind::Reinstall);
    assert_eq!(plan.start(), Start::Stored);
    assert_eq!(plan.build(), "200");
    let (updated, steps) = fx.run(&plan);
    assert_eq!(updated.kind, JournalKind::Reinstall);
    assert_updated(&fx);
    // The unchanged file was damaged: fixed, yours saved aside.
    assert_eq!(fx.read("Data/core.dat.wineo-saved"), damaged);
    assert_eq!(Fixture::saved(&updated), ["Data/core.dat"]);
    // With a trusted record, what the new build drops goes on a hash match.
    assert!(!fx.folder.join("empty.txt").exists());
    assert!(
        steps
            .iter()
            .any(|step| matches!(step, Progress::Verify { .. }))
    );
}

#[test]
fn a_reinstall_on_the_current_build_still_runs_and_reruns_setup() {
    let fx = Fixture::new();
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, None);
    fx.install();
    fx.damage("bin/Game.exe", "MZ the game itselX");
    let plan = fx.op(reinstall()).unwrap();
    assert_eq!(plan.installed_build(), Some("100"));
    assert_eq!(plan.build(), "100");
    let (updated, _) = fx.run(&plan);
    assert_eq!(updated.kind, JournalKind::Reinstall);
    assert_eq!(fx.read("bin/Game.exe"), "MZ the game itself");
    assert_eq!(fx.read("bin/Game.exe.wineo-saved"), "MZ the game itselX");
    assert_eq!(fx.names().len(), 4);
    assert_eq!(fx.base_build().as_deref(), Some("100"));
}

#[test]
fn a_reinstall_picks_up_newly_owned_dlc() {
    let fx = Fixture::new();
    fx.gog.lock().unwrap().dlc = vec![DLC];
    fx.gog.lock().unwrap().owned = Some(vec![GAME]);
    fx.content.add_build(
        Fixture::game("100").depot(FakeDepot::new(DLC).file(FakeFile::new("dlc.dat", b"bought"))),
    );
    fx.install();
    assert!(!fx.folder.join("dlc.dat").exists());
    fx.gog.lock().unwrap().owned = Some(vec![GAME, DLC]);
    let plan = fx.op(reinstall()).unwrap();
    assert_eq!(plan.products().len(), 2);
    fx.run(&plan);
    assert_eq!(fx.read("dlc.dat"), "bought");
    let state = fx.state();
    assert_eq!(
        state.committed.unwrap().products[1].status,
        ProductStatus::Installed
    );
    assert_whole(&fx, "100");
}

#[test]
fn a_reinstall_with_no_trusted_record_deletes_nothing() {
    let fx = Fixture::new();
    fx.installed_then(game_200());
    forget_record(&fx);
    let plan = fx.op(reinstall()).unwrap();
    assert_eq!(plan.start(), Start::Target);
    assert_eq!(plan.notes(), [Note::NothingDeleted]);
    fx.run(&plan);
    assert_eq!(fx.base_build().as_deref(), Some("200"));
    assert!(fx.folder.join("empty.txt").is_file());
}

#[test]
fn an_interrupted_reinstall_is_finished_as_a_reinstall() {
    for point in [
        "galaxy.update.journal.after",
        "galaxy.update.files.after",
        "galaxy.update.setup.after",
        "galaxy.update.installed.after",
    ] {
        let fx = Fixture::new();
        fx.installed_then(game_200());
        fx.damage("Data/core.dat", "core data, several chunks LONG");
        set_failpoint(Some(point));
        let error = fx.run_err(&fx.op(reinstall()).unwrap());
        set_failpoint(None);
        assert!(format!("{error:#}").contains("injected"), "{point}");
        assert_eq!(fx.state().journal.unwrap().kind, JournalKind::Reinstall);
        let plan = fx
            .op(Options {
                refresh: true,
                ..Options::default()
            })
            .unwrap();
        assert_eq!(plan.kind(), JournalKind::Reinstall, "{point}");
        let (updated, _) = fx.run(&plan);
        assert!(updated.recovered);
        assert_updated(&fx);
        assert_eq!(fx.read("Data/core.dat"), CORE);
    }
    // Stopped before the journal, a reinstall just starts again.
    let fx = Fixture::new();
    fx.installed_then(game_200());
    update_interrupted(&fx, "galaxy.update.pending.after", 0);
}

// Moving between tracks.

/// The installed game rewritten as an offline installer left it: the
/// installer track with versions and GOG's codes, and no Galaxy record or
/// support files.
fn as_installer_game(fx: &Fixture) {
    let lock = FolderLock::exclusive(&fx.paths, &fx.folder, &|| false).unwrap();
    let mut state = lock.read().unwrap().unwrap();
    let committed = state.committed.as_mut().unwrap();
    committed.track = Track::Installer;
    committed.language = "en".to_owned();
    for product in &mut committed.products {
        product.build = None;
        product.version = Some("1.0".to_owned());
        product.language = None;
    }
    lock.write(&state).unwrap();
    lock.remove_galaxy_files().unwrap();
}

#[test]
fn an_installer_game_moves_onto_galaxy_keeping_what_matches_and_deleting_nothing() {
    let fx = Fixture::new();
    fx.content.add_build(in_english(scripted("100")));
    with_setup(&fx, None, None);
    fx.install();
    as_installer_game(&fx);
    fx.gog.lock().unwrap().newest_first = true;
    let mut newer = in_english(game_200()).dependency("MSVC2017");
    newer.script_interpreter = true;
    fx.content.add_build(newer);
    // What the old track left, and files of yours.
    fx.damage("unins000.exe", "uninstaller");
    fx.damage("Data/core.dat", "shorter");
    fs::create_dir_all(fx.folder.join("new")).unwrap();
    fx.damage("new/fresh.dat", "brand new");
    let planned = plan_update(
        &fx.paths,
        Some(&mut fx.session()),
        "game",
        to_galaxy(),
        &Token::new(),
    )
    .unwrap();
    let PlannedUpdate::Galaxy(Planned::Update(plan)) = planned else {
        panic!("a move onto Galaxy is a Galaxy operation");
    };
    assert_eq!(plan.kind(), JournalKind::Reinstall);
    assert_eq!(plan.start(), Start::Target);
    assert_eq!(plan.build(), "200");
    assert_eq!(plan.installed_build(), None);
    assert_eq!(plan.target().track, Track::Galaxy);
    assert!(plan.notes().contains(&Note::NothingDeleted));
    let before = fx.chunk_count();
    let (updated, _) = fx.run(&plan);
    assert_eq!(updated.kind, JournalKind::Reinstall);
    let state = fx.state();
    assert_eq!(state.journal, None);
    let committed = state.committed.as_ref().unwrap();
    assert_eq!(committed.track, Track::Galaxy);
    assert_eq!(committed.products[0].build.as_deref(), Some("200"));
    // GOG's depot code beside the code the installer recorded.
    assert_eq!(committed.language, "en");
    assert_eq!(committed.products[0].language.as_deref(), Some("en-US"));
    let record = fx.record().unwrap();
    assert!(record::installed(&state, &record).unwrap().is_some());
    // Old-track extras stay, files of the old build the new one drops stay,
    // and the damaged file is saved aside.
    assert_eq!(fx.read("unins000.exe"), "uninstaller");
    assert!(fx.folder.join("empty.txt").is_file());
    assert_eq!(fx.read("Data/core.dat.wineo-saved"), "shorter");
    // Answer b: anything at a target path that isn't the target's is saved
    // aside, the old build's files included.
    assert_eq!(
        Fixture::saved(&updated),
        [
            "Data/big.dat",
            "Data/core.dat",
            "bin/Game.exe",
            &format!("goggame-{GAME}.info")
        ]
    );
    assert_eq!(fx.read("Data/core.dat"), CORE);
    // A file that already matched the new build wasn't fetched again.
    assert_eq!(fx.read("new/fresh.dat"), "brand new");
    assert!(!fx.chunks_since(before).contains(&chunk(b"brand ne")));
    // Every setup step ran, for the install and again for the move.
    assert_eq!(fx.names().len(), 4);
    assert!(!fx.launch_refused());
}

#[test]
fn a_move_onto_galaxy_refuses_a_game_with_no_generation_2_build() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    fx.install();
    as_installer_game(&fx);
    fx.hide("100");
    let mut old = Fixture::game("50");
    old.generation = 1;
    fx.content.add_build(old);
    let error = plan_update(
        &fx.paths,
        Some(&mut fx.session()),
        "game",
        to_galaxy(),
        &Token::new(),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "GOG lists no Galaxy build of Fake Game that wineo can install."
    );
    assert!(matches!(
        error.downcast_ref(),
        Some(GalaxyError::NoBuild { .. })
    ));
}

#[test]
fn an_unstarted_move_onto_galaxy_never_makes_a_file_tracked() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    fx.install();
    as_installer_game(&fx);
    fx.damage("Data/core.dat", "yours");
    let plan = fx.op(to_galaxy()).unwrap();
    set_failpoint(Some("galaxy.update.pending.after"));
    let error = fx.run_err(&plan);
    set_failpoint(None);
    assert!(format!("{error:#}").contains("injected"), "{error:#}");
    let state = fx.state();
    assert!(state.journal.is_none());
    assert_eq!(state.committed.as_ref().unwrap().track, Track::Installer);
    let record = fx.record().unwrap();
    // Pending alone is nothing: no journal, no installed.
    assert!(record::pending(&state, &record).is_none());
    assert!(record::installed(&state, &record).unwrap().is_none());
    let plan = fx.op(to_galaxy()).unwrap();
    assert!(!plan.is_recovery());
    assert_eq!(plan.start(), Start::Target);
    let (updated, _) = fx.run(&plan);
    assert_eq!(fx.read("Data/core.dat.wineo-saved"), "yours");
    assert_eq!(Fixture::saved(&updated), ["Data/core.dat"]);
    assert_whole(&fx, "100");
}

#[test]
fn an_interrupted_move_onto_galaxy_is_finished_as_a_reinstall() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    fx.install();
    as_installer_game(&fx);
    fx.damage("Data/core.dat", "yours");
    let plan = fx.op(to_galaxy()).unwrap();
    set_failpoint(Some("galaxy.update.files.after"));
    let error = fx.run_err(&plan);
    set_failpoint(None);
    assert!(format!("{error:#}").contains("injected"), "{error:#}");
    let state = fx.state();
    let journal = state.journal.as_ref().unwrap();
    assert_eq!(journal.kind, JournalKind::Reinstall);
    assert_eq!(journal.target.track, Track::Galaxy);
    // The journal wins over the options, and the dispatch follows its track.
    let planned = plan_update(
        &fx.paths,
        Some(&mut fx.session()),
        "game",
        Options {
            refresh: true,
            ..Options::default()
        },
        &Token::new(),
    )
    .unwrap();
    let PlannedUpdate::Galaxy(Planned::Update(plan)) = planned else {
        panic!("a Galaxy journal is finished on Galaxy");
    };
    assert_eq!(plan.kind(), JournalKind::Reinstall);
    assert!(plan.is_recovery());
    fx.run(&plan);
    assert_whole(&fx, "100");
    assert_eq!(fx.read("Data/core.dat.wineo-saved"), "yours");
}

// A record left by a move off Galaxy.

#[test]
fn a_sidecar_left_by_a_move_to_the_installer_track_pins_no_store_version() {
    let fx = Fixture::new();
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, None);
    fx.install();
    let version = fx.version_dir("MSVC2017");
    assert!(version.is_dir());
    // Committed on the installer track, the sidecar still there: a crash
    // between the commit and the removal.
    let lock = FolderLock::exclusive(&fx.paths, &fx.folder, &|| false).unwrap();
    let mut state = lock.read().unwrap().unwrap();
    state.committed.as_mut().unwrap().track = Track::Installer;
    lock.write(&state).unwrap();
    drop(lock);
    assert!(fx.record().is_some());
    // Reads ignore it.
    let record = fx.record().unwrap();
    assert!(record::installed(&fx.state(), &record).unwrap().is_none());
    assert!(record::all(&fx.paths.gog_folders_dir()).unwrap().is_empty());
    let removed = store::clean(&fx.paths).unwrap();
    assert!(!removed.is_empty());
    assert!(!version.exists());
}

#[test]
fn a_galaxy_folders_sidecar_still_pins_its_versions() {
    let fx = Fixture::new();
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, None);
    fx.install();
    let version = fx.version_dir("MSVC2017");
    assert_eq!(record::all(&fx.paths.gog_folders_dir()).unwrap().len(), 1);
    assert!(store::clean(&fx.paths).unwrap().is_empty());
    assert!(version.is_dir());
}
