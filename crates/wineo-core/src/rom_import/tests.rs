use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::time::{Duration, Instant};

use tempfile::{TempDir, tempdir};

use super::*;
use crate::config::{GlobalConfig, title_problem};
use crate::launch::{Launch, Overrides};
use crate::test_script::{KillOnDrop, write_script};

fn paths(home: &Path) -> Paths {
    Paths::from_lookup(|key| (key == "HOME").then(|| home.as_os_str().to_owned())).unwrap()
}

fn portable_paths(home: &Path) -> Paths {
    let portable = home.join("portable");
    fs::create_dir_all(&portable).unwrap();
    Paths::select(
        |key| (key == "HOME").then(|| home.as_os_str().to_owned()),
        Some(&portable),
    )
    .unwrap()
}

fn touch(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"").unwrap();
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A home with a `roms` folder.
fn home() -> (TempDir, Paths, PathBuf) {
    let home = tempdir().unwrap();
    let paths = paths(home.path());
    let roms = home.path().join("roms");
    fs::create_dir_all(&roms).unwrap();
    (home, paths, roms)
}

fn list(paths: &Paths, emulator: Emulator, path: &Path) -> Listing {
    listing(paths, &Scan::BuiltIn(emulator), path).unwrap()
}

/// The listed file names, in listing order.
fn names(listing: &Listing) -> Vec<&str> {
    listing
        .entries
        .iter()
        .map(|entry| entry.file_name.as_str())
        .collect()
}

fn plan_names(batch: &Batch) -> Vec<&str> {
    batch.ready.iter().map(|plan| plan.name.as_str()).collect()
}

/// An emulator file in the home folder, run as a picked install.
fn picked(home: &Path, emulator: Emulator) -> Emulating {
    let exe = home.join("Apps/emu");
    if !exe.exists() {
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        write_script(&exe, "#!/bin/sh\n");
    }
    Emulating::BuiltIn {
        emulator,
        install: Install::File(exe),
        fullscreen: true,
    }
}

fn existing(paths: &Paths, name: &str) {
    write(&paths.environment_config(name), "kind = \"native\"\n");
}

fn import_all(paths: &Paths, emulating: &Emulating, listing: Listing) -> BatchReport {
    let batch = plan_batch(
        paths,
        &GlobalConfig::default(),
        emulating,
        listing.entries,
        None,
    )
    .unwrap();
    assert!(batch.failures.is_empty(), "{:?}", batch.failures);
    apply_batch(paths, batch, &Token::new(), |_| Ok(())).unwrap()
}

#[test]
fn a_multi_track_cue_hides_every_track_it_names_and_nothing_else() {
    let (_home, paths, roms) = home();
    write(
        &roms.join("Game (USA).cue"),
        "FILE \"Game (USA) (Track 1).bin\" BINARY\n  TRACK 01 MODE2/2352\n\
         file Game (USA) (Track 2).bin BINARY\n  TRACK 02 AUDIO\n",
    );
    touch(&roms.join("Game (USA) (Track 1).bin"));
    touch(&roms.join("Game (USA) (Track 2).bin"));
    // Sharing the cue's name doesn't hide a file; naming it does.
    write(&roms.join("Other.cue"), "FILE \"data.img\" BINARY\n");
    touch(&roms.join("data.img"));
    touch(&roms.join("Other.bin"));
    let listing = list(&paths, Emulator::DuckStation, &roms);
    assert_eq!(
        names(&listing),
        ["Game (USA).cue", "Other.bin", "Other.cue"]
    );
}

#[test]
fn an_m3u_hides_the_files_it_names_in_a_subfolder() {
    let (_home, paths, roms) = home();
    write(
        &roms.join("Saga.m3u"),
        "# discs\n\ndiscs/Saga (Disc 1).chd\r\n  discs/Saga (Disc 2).chd\n",
    );
    touch(&roms.join("discs/Saga (Disc 1).chd"));
    touch(&roms.join("discs/Saga (Disc 2).chd"));
    touch(&roms.join("discs/Saga (Disc 3).chd"));
    let listing = list(&paths, Emulator::DuckStation, &roms);
    assert_eq!(names(&listing), ["Saga.m3u", "Saga (Disc 3).chd"]);
}

#[test]
fn an_m3u_naming_a_cue_hides_the_cue_and_its_tracks() {
    let (_home, paths, roms) = home();
    write(&roms.join("Epic.m3u"), "cues/Epic (Disc 1).cue\n");
    write(
        &roms.join("cues/Epic (Disc 1).cue"),
        "FILE \"../bins/Epic (Disc 1).bin\" BINARY\n",
    );
    touch(&roms.join("bins/Epic (Disc 1).bin"));
    let listing = list(&paths, Emulator::DuckStation, &roms);
    assert_eq!(names(&listing), ["Epic.m3u"]);
    // Also where the emulator's list has m3u but not cue.
    let scan = Scan::Custom {
        extensions: vec!["m3u".to_owned(), "bin".to_owned()],
        system: None,
    };
    let custom = listing_with(&paths, &scan, &roms);
    assert_eq!(names(&custom), ["Epic.m3u"]);
}

fn listing_with(paths: &Paths, scan: &Scan, path: &Path) -> Listing {
    listing(paths, scan, path).unwrap()
}

#[test]
fn a_cue_hides_nothing_for_an_emulator_that_does_not_list_cue() {
    let (_home, paths, roms) = home();
    write(&roms.join("Game.cue"), "FILE \"Game.bin\" BINARY\n");
    touch(&roms.join("Game.bin"));
    let listing = list(&paths, Emulator::Pcsx2, &roms);
    assert_eq!(names(&listing), ["Game.bin"]);
}

#[test]
fn a_ppsspp_folder_game_is_listed_once_and_not_looked_inside() {
    let (_home, paths, roms) = home();
    touch(&roms.join("Patapon (USA)/EBOOT.PBP"));
    touch(&roms.join("Patapon (USA)/extra/Inner.iso"));
    touch(&roms.join("Loose.iso"));
    let listing = list(&paths, Emulator::Ppsspp, &roms);
    assert_eq!(names(&listing), ["Loose.iso", "Patapon (USA)"]);
    let folder = &listing.entries[1];
    assert_eq!(folder.rom, roms.join("Patapon (USA)"));
    assert_eq!(folder.title, "Patapon");
    assert_eq!(folder.system, EntrySystem::Known("psp".to_owned()));
    // Picking the folder itself lists it as the one game.
    let picked = list(&paths, Emulator::Ppsspp, &roms.join("Patapon (USA)"));
    assert_eq!(names(&picked), ["Patapon (USA)"]);
}

#[test]
fn a_cemu_title_folder_is_a_game_and_an_update_or_dlc_folder_is_not() {
    let (_home, paths, roms) = home();
    let cemu_title = |folder: &str, title_id: &str| {
        let dir = roms.join(folder);
        write(
            &dir.join("code/app.xml"),
            &format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<app type=\"complex\" access=\"777\">\n  \
                 <title_id type=\"hexBinary\" length=\"8\">{title_id}</title_id>\n</app>\n"
            ),
        );
        touch(&dir.join("code/game.rpx"));
        fs::create_dir_all(dir.join("content")).unwrap();
        fs::create_dir_all(dir.join("meta")).unwrap();
    };
    cemu_title("Zelda [0005000010101C00]", "0005000010101C00");
    cemu_title("Zelda Demo", "0005000210101C00");
    // An update ships code of its own, .rpx and all.
    cemu_title("Zelda Update", "0005000E10101C00");
    cemu_title("Zelda DLC", "0005000C10101C00");
    cemu_title("Homebrew", "0005000F10101C00");
    cemu_title("Garbled", "not hex");
    // A 3DS-style id with a base type byte isn't a Wii U title.
    cemu_title("Not Wii U", "0004000010101C00");
    let missing = roms.join("No App Xml");
    touch(&missing.join("code/game.rpx"));
    // A base title id without Cemu's content/ and meta/ isn't a title folder.
    cemu_title("Half Copied", "0005000010101D00");
    fs::remove_dir(roms.join("Half Copied/content")).unwrap();
    let listing = list(&paths, Emulator::Cemu, &roms);
    assert_eq!(names(&listing), ["Zelda [0005000010101C00]", "Zelda Demo"]);
    assert_eq!(listing.entries[0].title, "Zelda");
    assert_eq!(
        listing.entries[0].system,
        EntrySystem::Known("wiiu".to_owned())
    );
}

#[test]
fn an_rpcs3_disc_folder_is_one_game() {
    let (_home, paths, roms) = home();
    touch(&roms.join("Demon's Souls/PS3_GAME/USRDIR/EBOOT.BIN"));
    touch(&roms.join("Demon's Souls/PS3_GAME/PARAM.SFO"));
    let listing = list(&paths, Emulator::Rpcs3, &roms);
    assert_eq!(names(&listing), ["Demon's Souls"]);
}

#[test]
fn extensions_match_in_any_case() {
    let (_home, paths, roms) = home();
    touch(&roms.join("Upper.ISO"));
    touch(&roms.join("Mixed.Iso"));
    touch(&roms.join("notes.txt"));
    let listing = list(&paths, Emulator::Pcsx2, &roms);
    assert_eq!(names(&listing), ["Mixed.Iso", "Upper.ISO"]);
}

#[test]
fn a_folder_link_loop_is_read_once() {
    let (_home, paths, roms) = home();
    touch(&roms.join("sub/Game.gba"));
    symlink(&roms, roms.join("sub/back")).unwrap();
    symlink(roms.join("sub"), roms.join("sub/again")).unwrap();
    // Bounded, since a scan that follows the loop wouldn't end.
    let (sent, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sent.send(list(&paths, Emulator::Mgba, &roms));
    });
    let listing = received
        .recv_timeout(Duration::from_secs(10))
        .expect("the scan followed the loop");
    assert_eq!(names(&listing), ["Game.gba"]);
    assert!(listing.skipped.is_empty(), "{:?}", listing.skipped);
}

#[test]
fn titles_drop_bracketed_notes_and_fall_back_when_nothing_is_left() {
    let (home, paths, roms) = home();
    touch(&roms.join("Metroid Prime (USA) (Rev 1) [!].rvz"));
    touch(&roms.join("[!] (USA).rvz"));
    touch(&roms.join("Open (Bracket.rvz"));
    touch(&roms.join("(Note) - .rvz"));
    let listing = list(&paths, Emulator::Dolphin, &roms);
    let titles: Vec<&str> = listing
        .entries
        .iter()
        .map(|entry| entry.title.as_str())
        .collect();
    assert_eq!(
        titles,
        ["(Note) -", "[!] (USA)", "Metroid Prime", "Open (Bracket"]
    );
    let dolphin = picked(home.path(), Emulator::Dolphin);
    let batch = plan_batch(
        &paths,
        &GlobalConfig::default(),
        &dolphin,
        listing.entries,
        Some("gamecube"),
    )
    .unwrap();
    assert_eq!(
        plan_names(&batch),
        ["note", "usa", "metroid-prime", "open-bracket"]
    );
}

#[test]
fn a_title_over_the_limit_is_cut_to_a_valid_title() {
    let (_home, paths, roms) = home();
    let long = "Word ".repeat(40);
    touch(&roms.join(format!("{long}(USA).gba")));
    let listing = list(&paths, Emulator::Mgba, &roms);
    let title = &listing.entries[0].title;
    assert_eq!(title.chars().count(), 119, "{title:?}");
    assert_eq!(title_problem(title), None);
}

#[test]
fn two_files_with_one_title_get_notes_and_distinct_names() {
    let (home, paths, roms) = home();
    touch(&roms.join("Tetris (USA).gb"));
    touch(&roms.join("Tetris (Europe).gb"));
    touch(&roms.join("Other.gb"));
    let listing = list(&paths, Emulator::Mgba, &roms);
    let notes: Vec<Option<&str>> = listing
        .entries
        .iter()
        .map(|entry| entry.note.as_deref())
        .collect();
    assert_eq!(
        notes,
        [None, Some("Tetris (Europe).gb"), Some("Tetris (USA).gb")]
    );
    let mgba = picked(home.path(), Emulator::Mgba);
    let batch = plan_batch(
        &paths,
        &GlobalConfig::default(),
        &mgba,
        listing.entries,
        None,
    )
    .unwrap();
    assert_eq!(plan_names(&batch), ["other", "tetris", "tetris-gb"]);
}

#[test]
fn a_name_an_environment_has_gets_the_system_then_a_number() {
    let (home, paths, roms) = home();
    for name in [
        "metroid-prime",
        "zelda",
        "zelda-wii",
        "zelda-wii-2",
        "unknown",
    ] {
        existing(&paths, name);
    }
    touch(&roms.join("Metroid Prime (USA).gcm"));
    touch(&roms.join("Zelda.wbfs"));
    touch(&roms.join("Unknown.iso"));
    let mut entries = list(&paths, Emulator::Dolphin, &roms).entries;
    // A game with no system gets just the number.
    entries[1].system = EntrySystem::Unknown;
    let dolphin = picked(home.path(), Emulator::Dolphin);
    let batch = plan_batch(&paths, &GlobalConfig::default(), &dolphin, entries, None).unwrap();
    assert_eq!(
        plan_names(&batch),
        ["metroid-prime-gamecube", "unknown-2", "zelda-wii-3"]
    );
}

#[test]
fn a_mixed_game_boy_folder_is_tagged_per_file() {
    let (home, paths, roms) = home();
    touch(&roms.join("Red.gb"));
    touch(&roms.join("Crystal.gbc"));
    touch(&roms.join("Emerald.gba"));
    let listing = list(&paths, Emulator::Mgba, &roms);
    let report = import_all(&paths, &picked(home.path(), Emulator::Mgba), listing);
    assert_eq!(report.done, ["crystal", "emerald", "red"]);
    for (name, system) in [("red", "gb"), ("crystal", "gbc"), ("emerald", "gba")] {
        let config = Environment::load(&paths, name).unwrap().config;
        assert_eq!(config.tags, [system]);
        assert_eq!(config.emulation.unwrap().system.as_deref(), Some(system));
    }
}

#[test]
fn dolphins_ambiguous_files_take_one_system_choice() {
    let (home, paths, roms) = home();
    touch(&roms.join("A.iso"));
    touch(&roms.join("B.rvz"));
    touch(&roms.join("C.gcm"));
    touch(&roms.join("D.wbfs"));
    let listing = list(&paths, Emulator::Dolphin, &roms);
    let systems: Vec<&EntrySystem> = listing.entries.iter().map(|entry| &entry.system).collect();
    assert_eq!(
        systems,
        [
            &EntrySystem::Ambiguous,
            &EntrySystem::Ambiguous,
            &EntrySystem::Known("gamecube".to_owned()),
            &EntrySystem::Known("wii".to_owned()),
        ]
    );
    let dolphin = picked(home.path(), Emulator::Dolphin);
    let error = plan_batch(
        &paths,
        &GlobalConfig::default(),
        &dolphin,
        listing.entries.clone(),
        None,
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Some games could be gamecube or wii; choose one."
    );
    let error = plan_batch(
        &paths,
        &GlobalConfig::default(),
        &dolphin,
        listing.entries.clone(),
        Some("ps2"),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "`ps2` isn't a system Dolphin plays: gamecube, wii."
    );
    // Without an ambiguous file, no choice is needed.
    let plain = listing.entries[2..].to_vec();
    assert_eq!(
        plan_batch(&paths, &GlobalConfig::default(), &dolphin, plain, None)
            .unwrap()
            .ready
            .len(),
        2
    );
    let batch = plan_batch(
        &paths,
        &GlobalConfig::default(),
        &dolphin,
        listing.entries,
        Some("wii"),
    )
    .unwrap();
    let systems: Vec<Option<&str>> = batch
        .ready
        .iter()
        .map(|plan| plan.system.as_deref())
        .collect();
    assert_eq!(
        systems,
        [Some("wii"), Some("wii"), Some("gamecube"), Some("wii")]
    );
}

#[test]
fn a_large_folder_lists_and_plans_quickly() {
    let (home, paths, roms) = home();
    for folder in 0..40 {
        let dir = roms.join(format!("set {folder}"));
        fs::create_dir_all(&dir).unwrap();
        for game in 0..100 {
            fs::write(dir.join(format!("Game {folder}-{game} (USA).gba")), b"").unwrap();
            fs::write(dir.join(format!("readme {game}.txt")), b"").unwrap();
        }
    }
    let started = Instant::now();
    let listing = list(&paths, Emulator::Mgba, &roms);
    let listed = started.elapsed();
    assert_eq!(listing.entries.len(), 4000);
    let mgba = picked(home.path(), Emulator::Mgba);
    let batch = plan_batch(
        &paths,
        &GlobalConfig::default(),
        &mgba,
        listing.entries,
        None,
    )
    .unwrap();
    assert_eq!(batch.ready.len(), 4000);
    let planned = started.elapsed();
    assert!(
        planned < Duration::from_secs(20),
        "listed in {listed:?}, planned in {planned:?}"
    );
    // A real-sized batch applies. Each import still rewrites library.toml,
    // so this stays small; that an import doesn't reload every environment
    // is pinned by `catching_up_on_imports_loads_only_new_environments`.
    let started = Instant::now();
    let some = Batch {
        ready: batch.ready[..300].to_vec(),
        ..Batch::default()
    };
    let report = apply_batch(&paths, some, &Token::new(), |_| Ok(())).unwrap();
    assert_eq!(report.done.len(), 300);
    assert!(
        started.elapsed() < Duration::from_secs(120),
        "applied in {:?}",
        started.elapsed()
    );
}

#[test]
fn an_unreadable_subfolder_is_skipped() {
    let (_home, paths, roms) = home();
    touch(&roms.join("Readable.gba"));
    let locked = roms.join("locked");
    touch(&locked.join("Hidden.gba"));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    // Root reads it anyway, so there is nothing to check.
    let readable_anyway = fs::read_dir(&locked).is_ok();
    let listing = list(&paths, Emulator::Mgba, &roms);
    let picked_itself = listing_error(&paths, &locked);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if readable_anyway {
        return;
    }
    assert_eq!(names(&listing), ["Readable.gba"]);
    assert_eq!(listing.skipped, [locked]);
    // The folder picked can't be read, which is an error.
    assert!(picked_itself.starts_with("Can't read "), "{picked_itself}");
}

fn listing_error(paths: &Paths, path: &Path) -> String {
    listing(paths, &Scan::BuiltIn(Emulator::Mgba), path)
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default()
}

#[test]
fn a_rerun_after_import_finds_everything_already_imported() {
    let (home, paths, roms) = home();
    touch(&roms.join("One.gba"));
    touch(&roms.join("Two.gba"));
    picked(home.path(), Emulator::Mgba);
    let custom = Emulating::Custom {
        exe: "~/Apps/emu".to_owned(),
        args: "{rom}".to_owned(),
    };
    let scan = Scan::Custom {
        extensions: vec!["GBA".to_owned()],
        system: None,
    };
    // Imported through a link to the folder, then found through the folder
    // and the link, whichever emulator imported it.
    let link = home.path().join("link");
    symlink(&roms, &link).unwrap();
    let report = import_all(&paths, &custom, listing(&paths, &scan, &link).unwrap());
    assert_eq!(report.done, ["one", "two"]);
    let imported = [
        &EntryState::AlreadyImported("one".to_owned()),
        &EntryState::AlreadyImported("two".to_owned()),
    ];
    let real = list(&paths, Emulator::Mgba, &roms);
    let states: Vec<&EntryState> = real.entries.iter().map(|entry| &entry.state).collect();
    assert_eq!(states, imported);
    let again = list(&paths, Emulator::Mgba, &link);
    let states: Vec<&EntryState> = again.entries.iter().map(|entry| &entry.state).collect();
    assert_eq!(states, imported);
    let mgba = picked(home.path(), Emulator::Mgba);
    let batch = plan_batch(&paths, &GlobalConfig::default(), &mgba, again.entries, None).unwrap();
    assert!(batch.ready.is_empty());
    let already: Vec<&str> = batch
        .already
        .iter()
        .map(|game| game.name.as_str())
        .collect();
    assert_eq!(already, ["one", "two"]);
}

#[test]
fn a_second_apply_of_the_same_rom_reports_it_already_imported() {
    let (home, paths, roms) = home();
    touch(&roms.join("Game.gba"));
    let mgba = picked(home.path(), Emulator::Mgba);
    let listing = list(&paths, Emulator::Mgba, &roms);
    let first = plan_batch(
        &paths,
        &GlobalConfig::default(),
        &mgba,
        listing.entries.clone(),
        None,
    )
    .unwrap();
    let mut second = plan_batch(
        &paths,
        &GlobalConfig::default(),
        &mgba,
        listing.entries,
        None,
    )
    .unwrap();
    // A second import that picked another name for the same ROM.
    second.ready[0].name = "game-again".to_owned();
    let report = apply_batch(&paths, first, &Token::new(), |_| Ok(())).unwrap();
    assert_eq!(report.done, ["game"]);
    let mut seen = Vec::new();
    let report = apply_batch(&paths, second, &Token::new(), |applying| {
        if let Applying::AlreadyImported(_, name) = applying {
            seen.push(name.to_owned());
        }
        Ok(())
    })
    .unwrap();
    assert!(report.done.is_empty());
    assert_eq!(report.already, ["game"]);
    assert_eq!(seen, ["game"]);
    assert_eq!(report.total, 0);
    assert_eq!(environment::names(&paths).unwrap(), ["game"]);
}

#[test]
fn a_name_taken_after_planning_is_picked_again() {
    let (home, paths, roms) = home();
    touch(&roms.join("A.gba"));
    touch(&roms.join("B.gba"));
    let mgba = picked(home.path(), Emulator::Mgba);
    let entries = list(&paths, Emulator::Mgba, &roms).entries;
    let batch = plan_batch(&paths, &GlobalConfig::default(), &mgba, entries, None).unwrap();
    // Taken after planning.
    existing(&paths, "a");
    let report = apply_batch(&paths, batch, &Token::new(), |_| Ok(())).unwrap();
    assert_eq!(report.done, ["a-gba", "b"]);
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    let config = Environment::load(&paths, "a-gba").unwrap().config;
    assert!(config.targets.contains_key("a-gba"));
}

#[test]
fn a_failed_game_is_reported_and_the_rest_go_on() {
    let (home, paths, roms) = home();
    touch(&roms.join("A.gba"));
    touch(&roms.join("B.gba"));
    let mgba = picked(home.path(), Emulator::Mgba);
    let entries = list(&paths, Emulator::Mgba, &roms).entries;
    let mut batch = plan_batch(&paths, &GlobalConfig::default(), &mgba, entries, None).unwrap();
    batch.ready[0].name = "not valid".to_owned();
    let report = apply_batch(&paths, batch, &Token::new(), |_| Ok(())).unwrap();
    assert_eq!(report.done, ["b"]);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].name.as_deref(), Some("not valid"));
    assert_eq!(
        report.failures[0].reason,
        "invalid name `not valid`: use letters, digits, `-`, and `_`"
    );
}

#[test]
fn retroarch_without_a_core_is_refused_per_game() {
    let (home, paths, roms) = home();
    touch(&roms.join("A.gba"));
    let listing = list(&paths, Emulator::Mgba, &roms);
    let retroarch = picked(home.path(), Emulator::RetroArch);
    let batch = plan_batch(
        &paths,
        &GlobalConfig::default(),
        &retroarch,
        listing.entries,
        None,
    )
    .unwrap();
    assert!(batch.ready.is_empty());
    assert_eq!(batch.failures[0].reason, "RetroArch needs a core.");
}

#[test]
fn a_portable_root_is_refused_in_listing_planning_and_applying() {
    let (home, paths, roms) = home();
    touch(&roms.join("Game.gba"));
    let mgba = picked(home.path(), Emulator::Mgba);
    let listing = list(&paths, Emulator::Mgba, &roms);
    let batch = plan_batch(
        &paths,
        &GlobalConfig::default(),
        &mgba,
        listing.entries.clone(),
        None,
    )
    .unwrap();
    let portable = portable_paths(home.path());
    let refused = "Emulated games can't be imported into a portable folder.";
    let error = super::listing(&portable, &Scan::BuiltIn(Emulator::Mgba), &roms).unwrap_err();
    assert_eq!(error.to_string(), refused);
    let error = plan_batch(
        &portable,
        &GlobalConfig::default(),
        &mgba,
        listing.entries,
        None,
    )
    .unwrap_err();
    assert_eq!(error.to_string(), refused);
    let error = apply(&portable, &batch.ready[0], &Token::new()).unwrap_err();
    assert_eq!(error.to_string(), refused);
    let error = apply_batch(&portable, batch, &Token::new(), |_| Ok(())).unwrap_err();
    assert_eq!(error.to_string(), refused);
    assert!(environment::names(&portable).unwrap().is_empty());
}

#[test]
fn the_written_config_reads_back_with_its_emulation_tags_title_and_target() {
    let (home, paths, roms) = home();
    let rom = roms.join("gamecube/Metroid Prime (USA).gcm");
    touch(&rom);
    let dolphin = picked(home.path(), Emulator::Dolphin);
    let report = import_all(&paths, &dolphin, list(&paths, Emulator::Dolphin, &roms));
    assert_eq!(report.done, ["metroid-prime"]);
    let environment = Environment::load(&paths, "metroid-prime").unwrap();
    let config = &environment.config;
    assert_eq!(config.kind.as_str(), "native");
    assert_eq!(config.title.as_deref(), Some("Metroid Prime"));
    assert_eq!(config.tags, ["gamecube"]);
    assert_eq!(
        config.emulation,
        Some(config::Emulation {
            emulator: "dolphin".to_owned(),
            system: Some("gamecube".to_owned()),
            rom: "~/roms/gamecube/Metroid Prime (USA).gcm".to_owned(),
        })
    );
    assert_eq!(config.targets.len(), 1);
    let target = &config.targets["metroid-prime"];
    assert_eq!(target.exe, "~/Apps/emu");
    assert_eq!(target.args, ["-b", "-e", rom.to_str().unwrap()]);
    assert!(target.default);
    assert!(!target.desktop);
    assert_eq!(target.name.as_deref(), Some("Metroid Prime"));
}

#[test]
fn a_custom_emulator_writes_custom_and_its_system_as_a_tag() {
    let (home, paths, roms) = home();
    touch(&roms.join("Odyssey.NSP"));
    touch(&roms.join("Other.xci"));
    picked(home.path(), Emulator::Mgba);
    let scan = Scan::Custom {
        extensions: vec![".nsp".to_owned(), " ".to_owned()],
        system: Some("Nintendo Switch".to_owned()),
    };
    let listing = listing(&paths, &scan, &roms).unwrap();
    assert_eq!(names(&listing), ["Odyssey.NSP"]);
    let custom = Emulating::Custom {
        exe: home.path().join("Apps/emu").to_str().unwrap().to_owned(),
        args: "-f -g {rom}".to_owned(),
    };
    import_all(&paths, &custom, listing);
    let config = Environment::load(&paths, "odyssey").unwrap().config;
    assert_eq!(config.tags, ["nintendo-switch"]);
    let emulation = config.emulation.unwrap();
    assert_eq!(emulation.emulator, "custom");
    assert_eq!(emulation.system.as_deref(), Some("nintendo-switch"));
    assert_eq!(config.targets["odyssey"].exe, "~/Apps/emu");

    assert_eq!(custom_system("  ").unwrap(), None);
    let error = custom_system("???").unwrap_err();
    assert_eq!(error.to_string(), "Invalid system: ???.");
    let refused = Scan::Custom {
        extensions: vec!["nsp".to_owned()],
        system: Some("???".to_owned()),
    };
    assert!(super::listing(&paths, &refused, &roms).is_err());
    let none = Scan::Custom {
        extensions: vec![".".to_owned()],
        system: None,
    };
    assert_eq!(
        super::listing(&paths, &none, &roms)
            .unwrap_err()
            .to_string(),
        "Give at least one file type for the emulator's games."
    );
}

#[test]
fn a_single_file_is_listed_alone_and_another_type_is_refused() {
    let (_home, paths, roms) = home();
    touch(&roms.join("Game (USA).gba"));
    touch(&roms.join("Other.gba"));
    let listing = list(&paths, Emulator::Mgba, &roms.join("Game (USA).gba"));
    assert_eq!(names(&listing), ["Game (USA).gba"]);
    assert!(listing.entries[0].matches("game"));
    assert!(listing.entries[0].matches("GAME (usa).gba"));
    assert!(!listing.entries[0].matches("Other"));
    touch(&roms.join("readme.txt"));
    assert_eq!(
        listing_error(&paths, &roms.join("readme.txt")),
        format!(
            "`{}` isn't a file mGBA plays.",
            roms.join("readme.txt").display()
        )
    );
}

#[test]
fn an_imported_game_launches_its_emulator_with_the_rom() {
    let (home, paths, roms) = home();
    let rom = roms.join("Space Game (USA).gba");
    touch(&rom);
    let ran = home.path().join("ran");
    let exe = home.path().join("Apps/emu");
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    write_script(
        &exe,
        format!(
            "#!/bin/sh\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\"; done > '{}'\n",
            ran.display()
        ),
    );
    let mgba = Emulating::BuiltIn {
        emulator: Emulator::Mgba,
        install: Install::File(exe),
        fullscreen: true,
    };
    let report = import_all(&paths, &mgba, list(&paths, Emulator::Mgba, &roms));
    assert_eq!(report.done, ["space-game"]);

    let environment = Environment::load(&paths, "space-game").unwrap();
    let launch = Launch::target(
        &paths,
        &GlobalConfig::default(),
        &environment,
        &Overrides::default(),
    )
    .unwrap();
    let mut child = KillOnDrop::new(launch.command().spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the emulator never ended");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success());
    assert_eq!(
        fs::read_to_string(&ran).unwrap(),
        format!("-f\n{}\n", rom.display())
    );
}

#[test]
fn a_byte_order_mark_doesnt_hide_a_sheets_first_line() {
    let (_home, paths, roms) = home();
    write(&roms.join("Game.cue"), "\u{feff}FILE \"Game.bin\" BINARY\n");
    touch(&roms.join("Game.bin"));
    write(&roms.join("Saga.m3u"), "\u{feff}Saga (Disc 1).chd\n");
    touch(&roms.join("Saga (Disc 1).chd"));
    let listing = list(&paths, Emulator::DuckStation, &roms);
    assert_eq!(names(&listing), ["Game.cue", "Saga.m3u"]);
}

#[test]
fn a_file_linked_from_two_places_is_listed_once_under_the_first_path() {
    let (_home, paths, roms) = home();
    touch(&roms.join("b/Game.gba"));
    fs::create_dir_all(roms.join("a")).unwrap();
    symlink(roms.join("b/Game.gba"), roms.join("a/Alias.gba")).unwrap();
    let listing = list(&paths, Emulator::Mgba, &roms);
    assert_eq!(names(&listing), ["Alias.gba"]);
    assert_eq!(listing.entries[0].rom, roms.join("a/Alias.gba"));
}

#[test]
fn sheet_lines_naming_absolute_missing_and_outside_files_hide_only_what_is_listed() {
    let (home, paths, roms) = home();
    let track = roms.join("tracks/Game (Track 1).bin");
    touch(&track);
    touch(&roms.join("tracks/Game (Track 2).bin"));
    write(
        &roms.join("Game.cue"),
        &format!(
            "FILE \"{}\" BINARY\nFILE \"gone.bin\" BINARY\n",
            track.display()
        ),
    );
    touch(&home.path().join("elsewhere/Outside.chd"));
    write(
        &roms.join("Saga.m3u"),
        "../elsewhere/Outside.chd\nmissing.chd\nInside.chd\n",
    );
    touch(&roms.join("Inside.chd"));
    let listing = list(&paths, Emulator::DuckStation, &roms);
    assert_eq!(
        names(&listing),
        ["Game.cue", "Game (Track 2).bin", "Saga.m3u"]
    );
}

#[test]
fn a_fifo_named_by_a_playlist_is_not_opened() {
    let (_home, paths, roms) = home();
    let status = std::process::Command::new("mkfifo")
        .arg(roms.join("Pipe.cue"))
        .status()
        .unwrap();
    assert!(status.success());
    write(&roms.join("Saga.m3u"), "Pipe.cue\n");
    let (sent, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let listing = list(&paths, Emulator::DuckStation, &roms);
        let _ = sent.send(names(&listing).join(","));
    });
    let names = received
        .recv_timeout(Duration::from_secs(10))
        .expect("the listing blocked on the FIFO");
    assert_eq!(names, "Saga.m3u");
}

#[test]
fn a_folder_that_can_be_read_but_not_searched_is_skipped() {
    let (_home, paths, roms) = home();
    touch(&roms.join("Readable.gba"));
    let locked = roms.join("locked");
    touch(&locked.join("Hidden.gba"));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o444)).unwrap();
    let searchable_anyway = fs::metadata(locked.join("Hidden.gba")).is_ok();
    let listing = list(&paths, Emulator::Mgba, &roms);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if searchable_anyway {
        return;
    }
    assert_eq!(names(&listing), ["Readable.gba"]);
    assert_eq!(listing.skipped, [locked]);
}

#[test]
fn compressed_images_and_eboot_files_get_their_game_names() {
    let (_home, paths, roms) = home();
    touch(&roms.join("Okami (USA).iso.gz"));
    touch(&roms.join("Plain.gz"));
    let titles = |listing: Listing| -> Vec<String> {
        listing
            .entries
            .into_iter()
            .map(|entry| entry.title)
            .collect()
    };
    assert_eq!(
        titles(list(&paths, Emulator::Pcsx2, &roms)),
        ["Okami", "Plain"]
    );
    let ps1 = roms.join("ps1");
    touch(&ps1.join("Final Fantasy VII (USA)/EBOOT.PBP"));
    assert_eq!(
        titles(list(&paths, Emulator::DuckStation, &ps1)),
        ["Final Fantasy VII"]
    );
}

#[test]
fn names_fold_accents_and_a_title_with_none_is_game() {
    let (home, paths, roms) = home();
    touch(&roms.join("Pokémon Snap (USA).gba"));
    touch(&roms.join("ハーデス (Japan).gba"));
    touch(&roms.join("ゼルダ (Japan).gbc"));
    // Multi-byte and over the limit, short enough in bytes for a file name.
    let long = "ポaa ".repeat(32);
    touch(&roms.join(format!("{long}.gb")));
    let listing = list(&paths, Emulator::Mgba, &roms);
    let long_title = listing
        .entries
        .iter()
        .find(|entry| entry.file_name.starts_with("ポaa"))
        .unwrap()
        .title
        .clone();
    assert_eq!(long_title.chars().count(), 119);
    assert_eq!(title_problem(&long_title), None);
    let mgba = picked(home.path(), Emulator::Mgba);
    let entries = listing
        .entries
        .into_iter()
        .filter(|entry| entry.title != long_title);
    let batch = plan_batch(&paths, &GlobalConfig::default(), &mgba, entries, None).unwrap();
    let named: Vec<(&str, &str)> = batch
        .ready
        .iter()
        .map(|plan| (plan.title.as_str(), plan.name.as_str()))
        .collect();
    assert_eq!(
        named,
        [
            ("Pokémon Snap", "pokemon-snap"),
            ("ゼルダ", "game"),
            ("ハーデス", "game-gba"),
        ]
    );
}

#[test]
fn apply_waits_for_another_import_holding_the_lock() {
    let (home, paths, roms) = home();
    touch(&roms.join("Game.gba"));
    let mgba = picked(home.path(), Emulator::Mgba);
    let entries = list(&paths, Emulator::Mgba, &roms).entries;
    let batch = plan_batch(&paths, &GlobalConfig::default(), &mgba, entries, None).unwrap();
    let plan = batch.ready[0].clone();
    let held = Locks::new(&paths).emulated_roms().unwrap();
    let (waiting, waited) = std::sync::mpsc::channel();
    let importer = {
        let paths = paths.clone();
        std::thread::spawn(move || {
            crate::operation::lock_wait_hooks::on_wait(move || waiting.send(()).unwrap());
            apply(&paths, &plan, &Token::new()).map(|outcome| match outcome {
                ApplyOutcome::Imported(applied) => applied.environment.name,
                ApplyOutcome::AlreadyImported(name) => format!("already {name}"),
            })
        })
    };
    waited
        .recv_timeout(Duration::from_secs(10))
        .expect("apply never waited on the lock");
    assert!(!paths.environment_config("game").exists());
    drop(held);
    assert_eq!(importer.join().unwrap().unwrap(), "game");
}

#[test]
fn canceling_a_batch_stops_before_the_next_game() {
    let (home, paths, roms) = home();
    touch(&roms.join("A.gba"));
    touch(&roms.join("B.gba"));
    touch(&roms.join("C.gba"));
    let mgba = picked(home.path(), Emulator::Mgba);
    let entries = list(&paths, Emulator::Mgba, &roms).entries;
    let batch = plan_batch(&paths, &GlobalConfig::default(), &mgba, entries, None).unwrap();
    let cancel = Token::new();
    let mut starts = 0;
    let report = apply_batch(&paths, batch, &cancel, |applying| {
        if let Applying::Starting(_) = applying {
            starts += 1;
            if starts == 2 {
                cancel.cancel();
            }
        }
        Ok(())
    })
    .unwrap();
    assert!(report.canceled);
    assert_eq!(report.done, ["a"]);
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(environment::names(&paths).unwrap(), ["a"]);
}

#[test]
fn a_custom_emulators_command_is_checked_when_planning() {
    let (home, paths, roms) = home();
    touch(&roms.join("Game.nsp"));
    let bin = home.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    write_script(bin.join("eden"), "#!/bin/sh\n");
    write(&bin.join("plain"), "not executable");
    let mut global = GlobalConfig::default();
    global
        .env
        .insert("PATH".to_owned(), bin.to_str().unwrap().to_owned());
    let scan = Scan::Custom {
        extensions: vec!["nsp".to_owned()],
        system: None,
    };
    let entries = listing(&paths, &scan, &roms).unwrap().entries;
    let plan = |exe: &str| {
        let custom = Emulating::Custom {
            exe: exe.to_owned(),
            args: "{rom}".to_owned(),
        };
        plan_batch(&paths, &global, &custom, entries.clone(), None)
            .map(|batch| batch.ready[0].target.exe.clone())
            .map_err(|error| error.to_string())
    };
    assert_eq!(plan("eden"), Ok("eden".to_owned()));
    // Stored as checked, so a stray space can't pass here and fail at launch.
    assert_eq!(plan(" eden "), Ok("eden".to_owned()));
    assert_eq!(plan("~/bin/eden"), Ok("~/bin/eden".to_owned()));
    assert_eq!(
        plan("missing"),
        Err("`missing` was not found on PATH".to_owned())
    );
    assert_eq!(
        plan("bin/eden"),
        Err("invalid exe `bin/eden`: use an absolute path or one starting with ~/".to_owned())
    );
    assert_eq!(
        plan("~/bin/gone"),
        Err("`~/bin/gone` does not exist".to_owned())
    );
    assert_eq!(
        plan("~/bin/plain"),
        Err(format!(
            "{} is not executable. Run chmod +x on it.",
            "~/bin/plain"
        ))
    );
    assert_eq!(
        plan("~/bin/eden.exe"),
        Err("`~/bin/eden.exe` is a Windows program, not a native one.".to_owned())
    );
    assert_eq!(
        plan(" "),
        Err("The emulator's command is empty.".to_owned())
    );
}

#[test]
fn catching_up_on_imports_loads_only_new_environments() {
    let (home, paths, roms) = home();
    touch(&roms.join("A.gba"));
    touch(&roms.join("B.gba"));
    let mgba = picked(home.path(), Emulator::Mgba);
    let entries = list(&paths, Emulator::Mgba, &roms).entries;
    let mut batch = plan_batch(&paths, &GlobalConfig::default(), &mgba, entries, None).unwrap();
    let b = batch.ready.pop().unwrap();
    apply_batch(&paths, batch, &Token::new(), |_| Ok(())).unwrap();
    let mut imported = Imported::read(&paths).unwrap();
    assert_eq!(imported.get(&roms.join("A.gba")), Some(&"a".to_owned()));
    // An environment already read isn't read again, so an edit to it
    // doesn't show; a new one does, and one that's gone is dropped.
    let config = paths.environment_config("a");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(&config, text.replace("A.gba", "Moved.gba")).unwrap();
    assert!(matches!(
        apply(&paths, &b, &Token::new()).unwrap(),
        ApplyOutcome::Imported(_)
    ));
    imported.refresh(&paths).unwrap();
    assert_eq!(imported.get(&roms.join("A.gba")), Some(&"a".to_owned()));
    assert_eq!(imported.get(&roms.join("Moved.gba")), None);
    assert_eq!(imported.get(&roms.join("B.gba")), Some(&"b".to_owned()));
    fs::remove_file(&config).unwrap();
    imported.refresh(&paths).unwrap();
    assert_eq!(imported.get(&roms.join("A.gba")), None);
}

fn core(path: &Path, info: Option<&str>) -> Core {
    touch(path);
    Core {
        path: path.to_owned(),
        info: info.map(emulator::parse_info),
    }
}

const MGBA_INFO: &str = "display_name = \"mGBA\"\nsystemid = \"game_boy_advance\"\n\
    supported_extensions = \"gb|gbc|gba\"\n\
    database = \"Nintendo - Game Boy|Nintendo - Game Boy Color|Nintendo - Game Boy Advance\"\n";
const PSX_INFO: &str = "display_name = \"Beetle PSX\"\nsystemid = \"playstation\"\n\
    supported_extensions = \"cue|m3u|bin\"\ndatabase = \"Sony - PlayStation\"\n";
const SNES_INFO: &str = "display_name = \"Snes9x\"\nsystemid = \"super_nes\"\n\
    supported_extensions = \"sfc|smc\"\ndatabase = \"Nintendo - SNES|Nintendo - Satellaview\"\n";

fn retroarch_scan(paths: &Paths, core: &Core, path: &Path) -> Listing {
    listing(paths, &Scan::RetroArch(core.clone()), path).unwrap()
}

fn systems(listing: &Listing) -> Vec<(&str, &EntrySystem)> {
    listing
        .entries
        .iter()
        .map(|entry| (entry.file_name.as_str(), &entry.system))
        .collect()
}

fn known(system: &str) -> EntrySystem {
    EntrySystem::Known(system.to_owned())
}

#[test]
fn a_retroarch_scan_lists_the_cores_files_tagged_per_file() {
    let (home, paths, roms) = home();
    let mgba = core(&home.path().join("cores/mgba_libretro.so"), Some(MGBA_INFO));
    touch(&roms.join("Tetris (World) [!].gb"));
    touch(&roms.join("sub/Pokemon Crystal (USA).GBC"));
    touch(&roms.join("Metroid Fusion (USA).gba"));
    touch(&roms.join("Super Metroid.sfc"));
    touch(&roms.join("notes.txt"));
    let listing = retroarch_scan(&paths, &mgba, &roms);
    assert_eq!(
        systems(&listing),
        [
            ("Metroid Fusion (USA).gba", &known("gba")),
            ("Pokemon Crystal (USA).GBC", &known("gbc")),
            ("Tetris (World) [!].gb", &known("gb")),
        ]
    );
    assert_eq!(listing.entries[2].title, "Tetris");

    // A core of one system tags everything with it, and the cue and m3u
    // rules hold as for any emulator.
    let psx = core(&home.path().join("cores/psx_libretro.so"), Some(PSX_INFO));
    let ps1 = roms.join("ps1");
    write(
        &ps1.join("Game (Disc 1).cue"),
        "FILE \"Game (Disc 1).bin\" BINARY\n",
    );
    touch(&ps1.join("Game (Disc 1).bin"));
    write(
        &ps1.join("Game (Disc 2).cue"),
        "FILE \"Game (Disc 2).bin\" BINARY\n",
    );
    touch(&ps1.join("Game (Disc 2).bin"));
    write(
        &ps1.join("Game.m3u"),
        "Game (Disc 1).cue\nGame (Disc 2).cue\n",
    );
    touch(&ps1.join("Loose.bin"));
    let listing = retroarch_scan(&paths, &psx, &ps1);
    assert_eq!(
        systems(&listing),
        [("Game.m3u", &known("ps1")), ("Loose.bin", &known("ps1"))]
    );

    // A core whose files it can't place leaves them untagged.
    let snes = core(&home.path().join("cores/snes_libretro.so"), Some(SNES_INFO));
    touch(&roms.join("snes/Game.smc"));
    let listing = retroarch_scan(&paths, &snes, &roms.join("snes"));
    assert_eq!(systems(&listing), [("Game.smc", &known("snes"))]);
    let odd = core(
        &home.path().join("cores/odd_libretro.so"),
        Some("systemid = \"arcade\"\nsupported_extensions = \"zip\"\n"),
    );
    touch(&roms.join("arcade/pacman.zip"));
    let listing = retroarch_scan(&paths, &odd, &roms.join("arcade"));
    assert_eq!(systems(&listing), [("pacman.zip", &EntrySystem::Unknown)]);

    // A single file the core doesn't play is refused.
    let error = super::listing(
        &paths,
        &Scan::RetroArch(mgba.clone()),
        &roms.join("notes.txt"),
    )
    .unwrap_err()
    .to_string();
    assert_eq!(
        error,
        format!(
            "`{}` isn't a file mGBA plays.",
            roms.join("notes.txt").display()
        )
    );
}

#[test]
fn a_retroarch_core_without_info_lists_every_file() {
    let (home, paths, roms) = home();
    let bare = core(&home.path().join("cores/bare_libretro.so"), None);
    touch(&roms.join("A.gba"));
    touch(&roms.join("B.weird"));
    touch(&roms.join("sub/README"));
    write(&roms.join("Disc.cue"), "FILE \"Disc.bin\" BINARY\n");
    touch(&roms.join("Disc.bin"));
    let listing = retroarch_scan(&paths, &bare, &roms);
    assert_eq!(
        systems(&listing),
        [
            ("A.gba", &EntrySystem::Unknown),
            ("B.weird", &EntrySystem::Unknown),
            ("Disc.cue", &EntrySystem::Unknown),
            ("README", &EntrySystem::Unknown),
        ]
    );
    assert_eq!(
        retroarch_scan(&paths, &bare, &roms.join("B.weird"))
            .entries
            .len(),
        1
    );
}

#[test]
fn the_cores_offered_are_the_ones_that_play_the_folders_files() {
    let (home, paths, roms) = home();
    let cores_dir = home.path().join("cores");
    let cores = [
        core(&cores_dir.join("bare_libretro.so"), None),
        core(&cores_dir.join("mgba_libretro.so"), Some(MGBA_INFO)),
        core(&cores_dir.join("psx_libretro.so"), Some(PSX_INFO)),
        core(&cores_dir.join("snes_libretro.so"), Some(SNES_INFO)),
        core(
            &cores_dir.join("2048_libretro.so"),
            Some("display_name = \"2048\"\nsystemid = \"2048\"\n"),
        ),
    ];
    let offered = |path: &Path| -> Vec<String> {
        matching_cores(&paths, &cores, path)
            .unwrap()
            .iter()
            .map(|core| core.id().to_owned())
            .collect()
    };
    fs::create_dir_all(roms.join("empty")).unwrap();
    assert!(offered(&roms.join("empty")).is_empty());
    touch(&roms.join("gba/deep/Game.GBA"));
    assert_eq!(offered(&roms.join("gba")), ["bare", "mgba"]);
    touch(&roms.join("mixed/Game.sfc"));
    touch(&roms.join("mixed/Game.gb"));
    assert_eq!(offered(&roms.join("mixed")), ["bare", "mgba", "snes"]);
    assert_eq!(offered(&roms.join("mixed/Game.sfc")), ["bare", "snes"]);
    touch(&roms.join("other/notes"));
    assert_eq!(offered(&roms.join("other")), ["bare"]);
    assert!(matching_cores(&paths, &cores, &roms.join("missing")).is_err());

    let portable = portable_paths(home.path());
    assert_eq!(
        matching_cores(&portable, &cores, &roms.join("gba"))
            .unwrap_err()
            .to_string(),
        "Emulated games can't be imported into a portable folder."
    );
}

fn retroarch(home: &Path, core: Core) -> Emulating {
    let exe = home.join("Apps/retroarch");
    if !exe.exists() {
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        write_script(&exe, "#!/bin/sh\n");
    }
    Emulating::RetroArch {
        install: Install::File(exe),
        core,
        fullscreen: true,
    }
}

const QUIT_ON: &str = "quit_on_close_content = \"2\"\nconfig_save_on_exit = \"false\"\n";

#[test]
fn a_retroarch_import_writes_its_target_and_the_quit_file() {
    let (home, paths, roms) = home();
    let mgba = core(&home.path().join("cores/mgba_libretro.so"), Some(MGBA_INFO));
    let rom = roms.join("Metroid Fusion (USA).gba");
    touch(&rom);
    touch(&roms.join("Tetris.gb"));
    let emulating = retroarch(home.path(), mgba.clone());
    let quit = emulator::quit_file(&paths);
    let listing = retroarch_scan(&paths, &mgba, &roms);
    let batch = plan_batch(
        &paths,
        &GlobalConfig::default(),
        &emulating,
        listing.entries,
        None,
    )
    .unwrap();
    assert!(batch.failures.is_empty(), "{:?}", batch.failures);
    // Planning writes nothing.
    assert!(!quit.exists());
    let report = apply_batch(&paths, batch, &Token::new(), |_| Ok(())).unwrap();
    assert_eq!(report.done, ["metroid-fusion", "tetris"]);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(fs::read_to_string(&quit).unwrap(), QUIT_ON);

    let config = Environment::load(&paths, "metroid-fusion").unwrap().config;
    assert_eq!(config.tags, ["gba"]);
    let emulation = config.emulation.unwrap();
    assert_eq!(emulation.emulator, "retroarch");
    assert_eq!(emulation.system.as_deref(), Some("gba"));
    let target = &config.targets["metroid-fusion"];
    assert_eq!(target.exe, "~/Apps/retroarch");
    assert_eq!(
        target.args,
        [
            "--appendconfig",
            quit.to_str().unwrap(),
            "-f",
            "-L",
            mgba.path.to_str().unwrap(),
            rom.to_str().unwrap(),
        ]
    );
    assert_eq!(
        Environment::load(&paths, "tetris").unwrap().config.tags,
        ["gb"]
    );

    // The setting turned off empties it at the next import, and the same
    // ROM counts as imported whichever core found it.
    write(
        &paths.global_config(),
        "[retroarch]\nquit_with_game = false\n",
    );
    touch(&roms.join("New.gba"));
    let report = import_all(&paths, &emulating, retroarch_scan(&paths, &mgba, &roms));
    assert_eq!(report.done, ["new"]);
    assert_eq!(fs::read_to_string(&quit).unwrap(), "");
    let bare = core(&home.path().join("cores/bare_libretro.so"), None);
    let again = retroarch_scan(&paths, &bare, &roms);
    assert!(
        again
            .entries
            .iter()
            .all(|entry| matches!(entry.state, EntryState::AlreadyImported(_)))
    );

    // A single apply writes it too, and a failed write is only a warning.
    fs::remove_file(&quit).unwrap();
    touch(&roms.join("Single.gba"));
    let listing = retroarch_scan(&paths, &mgba, &roms.join("Single.gba"));
    let batch = plan_batch(
        &paths,
        &GlobalConfig::default(),
        &emulating,
        listing.entries,
        None,
    )
    .unwrap();
    let ApplyOutcome::Imported(applied) = apply(&paths, &batch.ready[0], &Token::new()).unwrap()
    else {
        panic!("not imported");
    };
    assert!(applied.warnings.is_empty(), "{:?}", applied.warnings);
    assert_eq!(fs::read_to_string(&quit).unwrap(), "");
    fs::remove_file(&quit).unwrap();
    fs::create_dir(&quit).unwrap();
    touch(&roms.join("Blocked.gba"));
    let report = import_all(
        &paths,
        &emulating,
        retroarch_scan(&paths, &mgba, &roms.join("Blocked.gba")),
    );
    assert_eq!(report.done, ["blocked"]);
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(
        report.warnings[0].starts_with("Can't write RetroArch's quit file: "),
        "{:?}",
        report.warnings
    );
}

#[test]
fn other_emulators_imports_dont_write_the_quit_file() {
    let (home, paths, roms) = home();
    touch(&roms.join("Game.gba"));
    let mgba = picked(home.path(), Emulator::Mgba);
    let report = import_all(&paths, &mgba, list(&paths, Emulator::Mgba, &roms));
    assert_eq!(report.done, ["game"]);
    assert!(!emulator::quit_file(&paths).exists());
}

/// Prepares `name`'s launch as `wineo run` does, runs its command to the
/// end, and returns the events it sent.
fn launch_to_end(paths: &Paths, name: &str) -> Vec<String> {
    let mut events = Vec::new();
    let prepared = crate::launch::prepare(
        paths,
        name,
        &Overrides::default(),
        &Token::new(),
        crate::hooks::Output::Inherit,
        |event| events.push(format!("{event:?}")),
    )
    .unwrap();
    let mut child = KillOnDrop::new(prepared.launch.command().spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the emulator never ended");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success());
    events
}

#[test]
fn launching_a_retroarch_game_writes_the_quit_file_and_others_dont() {
    let (home, paths, roms) = home();
    let rom = roms.join("Game.gba");
    touch(&rom);
    touch(&roms.join("mgba/Other.gba"));
    let ran = home.path().join("ran");
    let exe = home.path().join("Apps/retroarch");
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    write_script(
        &exe,
        format!(
            "#!/bin/sh\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\"; done > '{}'\n",
            ran.display()
        ),
    );
    let mgba_core = core(&home.path().join("cores/mgba_libretro.so"), Some(MGBA_INFO));
    let emulating = retroarch(home.path(), mgba_core.clone());
    let report = import_all(&paths, &emulating, retroarch_scan(&paths, &mgba_core, &rom));
    assert_eq!(report.done, ["game"]);
    let mgba = picked(home.path(), Emulator::Mgba);
    let report = import_all(
        &paths,
        &mgba,
        list(&paths, Emulator::Mgba, &roms.join("mgba")),
    );
    assert_eq!(report.done, ["other"]);

    let quit = emulator::quit_file(&paths);
    fs::remove_file(&quit).unwrap();
    let events = launch_to_end(&paths, "other");
    assert!(!quit.exists(), "{events:?}");

    let events = launch_to_end(&paths, "game");
    assert!(
        !events.iter().any(|event| event.starts_with("Warning")),
        "{events:?}"
    );
    assert_eq!(fs::read_to_string(&quit).unwrap(), QUIT_ON);
    assert_eq!(
        fs::read_to_string(&ran).unwrap(),
        format!(
            "--appendconfig\n{}\n-f\n-L\n{}\n{}\n",
            quit.display(),
            mgba_core.path.display(),
            rom.display()
        )
    );

    // The setting reaches the next launch without touching the target.
    let before = fs::read_to_string(paths.environment_config("game")).unwrap();
    write(
        &paths.global_config(),
        "[retroarch]\nquit_with_game = false\n",
    );
    launch_to_end(&paths, "game");
    assert_eq!(fs::read_to_string(&quit).unwrap(), "");
    assert_eq!(
        fs::read_to_string(paths.environment_config("game")).unwrap(),
        before
    );

    // A file that can't be written is a warning, and the game still runs.
    fs::remove_file(&quit).unwrap();
    fs::create_dir(&quit).unwrap();
    fs::remove_file(&ran).unwrap();
    let events = launch_to_end(&paths, "game");
    assert!(
        events
            .iter()
            .any(|event| event.starts_with("Warning(\"Can't write RetroArch's quit file: ")),
        "{events:?}"
    );
    assert!(ran.exists());
}

#[test]
fn cores_are_matched_against_every_file_in_the_folder_as_listed() {
    let (home, paths, roms) = home();
    let cores_dir = home.path().join("cores");
    // Plays only what a cue sheet would hide, or a type no emulator lists.
    let bin_only = core(
        &cores_dir.join("bin_libretro.so"),
        Some("display_name = \"Bin\"\nsupported_extensions = \"bin\"\n"),
    );
    let odd = core(
        &cores_dir.join("odd_libretro.so"),
        Some("display_name = \"Odd\"\nsupported_extensions = \"xyz\"\n"),
    );
    let cores = [bin_only, odd];
    write(&roms.join("Disc.cue"), "FILE \"Disc.bin\" BINARY\n");
    touch(&roms.join("Disc.bin"));
    touch(&roms.join("deep/Thing.XYZ"));
    let offered: Vec<String> = matching_cores(&paths, &cores, &roms)
        .unwrap()
        .iter()
        .map(|core| core.id().to_owned())
        .collect();
    assert_eq!(offered, ["bin", "odd"]);
}

#[test]
fn a_custom_command_is_checked_like_a_native_exe() {
    let home = tempdir().unwrap();
    let paths = paths(home.path());
    let bin = home.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(home.path().join("apps")).unwrap();
    write_script(bin.join("eden"), "#!/bin/sh\nexit 0\n");
    write(&home.path().join("apps/plain"), "not a program\n");
    let path_var = bin.as_os_str();
    let refusal = |exe: &str| {
        check_custom_command(&paths, exe, path_var)
            .unwrap_err()
            .to_string()
    };

    for empty in ["", "  "] {
        assert_eq!(refusal(empty), "The emulator's command is empty.");
    }
    for windows in [
        "~/Games/setup.exe",
        r"C:\Games\eden",
        "eden.BAT",
        " eden.msi ",
    ] {
        assert_eq!(
            refusal(windows),
            format!(
                "`{}` is a Windows program, not a native one.",
                windows.trim()
            ),
        );
    }
    let missing = check_custom_command(&paths, "dolphin-emu", path_var).unwrap_err();
    assert!(
        matches!(
            missing.downcast_ref::<crate::Error>(),
            Some(crate::Error::NotOnPath(name)) if name == "dolphin-emu"
        ),
        "{missing:#}"
    );
    let plain = check_custom_command(&paths, "~/apps/plain", path_var).unwrap_err();
    assert!(
        matches!(
            plain.downcast_ref::<crate::Error>(),
            Some(crate::Error::NotExecutable(_))
        ),
        "{plain:#}"
    );

    // A bare name on PATH, a path from home, and an absolute path all pass.
    write_script(home.path().join("apps/eden"), "#!/bin/sh\nexit 0\n");
    for good in [
        "eden",
        "~/apps/eden",
        &home.path().join("apps/eden").display().to_string(),
    ] {
        check_custom_command(&paths, good, path_var).unwrap();
    }
}
