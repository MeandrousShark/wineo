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
    let batch = plan_batch(paths, emulating, listing.entries, None).unwrap();
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
fn a_cemu_title_folder_is_a_game_and_an_update_folder_is_not() {
    let (_home, paths, roms) = home();
    let title = roms.join("Zelda [0005000010101C00]");
    touch(&title.join("code/U-King.rpx"));
    fs::create_dir_all(title.join("content")).unwrap();
    fs::create_dir_all(title.join("meta")).unwrap();
    let update = roms.join("Zelda Update");
    touch(&update.join("code/app.xml"));
    fs::create_dir_all(update.join("content")).unwrap();
    fs::create_dir_all(update.join("meta")).unwrap();
    let dlc = roms.join("Zelda DLC");
    fs::create_dir_all(dlc.join("content")).unwrap();
    fs::create_dir_all(dlc.join("meta")).unwrap();
    let listing = list(&paths, Emulator::Cemu, &roms);
    assert_eq!(names(&listing), ["Zelda [0005000010101C00]"]);
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
    let listing = list(&paths, Emulator::Mgba, &roms);
    assert_eq!(names(&listing), ["Game.gba"]);
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
    let batch = plan_batch(&paths, &dolphin, listing.entries, Some("gamecube")).unwrap();
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
    let batch = plan_batch(&paths, &mgba, listing.entries, None).unwrap();
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
    let batch = plan_batch(&paths, &dolphin, entries, None).unwrap();
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
    let error = plan_batch(&paths, &dolphin, listing.entries.clone(), None).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Choose gamecube or wii for the games whose files could be either."
    );
    let error = plan_batch(&paths, &dolphin, listing.entries.clone(), Some("ps2")).unwrap_err();
    assert_eq!(
        error.to_string(),
        "`ps2` isn't a system Dolphin plays: gamecube, wii."
    );
    // Without an ambiguous file, no choice is needed.
    let plain = listing.entries[2..].to_vec();
    assert_eq!(
        plan_batch(&paths, &dolphin, plain, None)
            .unwrap()
            .ready
            .len(),
        2
    );
    let batch = plan_batch(&paths, &dolphin, listing.entries, Some("wii")).unwrap();
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
    let batch = plan_batch(&paths, &mgba, listing.entries, None).unwrap();
    assert_eq!(batch.ready.len(), 4000);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "listed in {listed:?}, planned in {:?}",
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
    let batch = plan_batch(&paths, &mgba, again.entries, None).unwrap();
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
    let first = plan_batch(&paths, &mgba, listing.entries.clone(), None).unwrap();
    let mut second = plan_batch(&paths, &mgba, listing.entries, None).unwrap();
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
fn a_failed_game_is_reported_and_the_rest_go_on() {
    let (home, paths, roms) = home();
    touch(&roms.join("A.gba"));
    touch(&roms.join("B.gba"));
    let mgba = picked(home.path(), Emulator::Mgba);
    let entries = list(&paths, Emulator::Mgba, &roms).entries;
    let batch = plan_batch(&paths, &mgba, entries, None).unwrap();
    // Taken after planning.
    existing(&paths, "a");
    let report = apply_batch(&paths, batch, &Token::new(), |_| Ok(())).unwrap();
    assert_eq!(report.done, ["b"]);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].name.as_deref(), Some("a"));
    assert_eq!(report.failures[0].reason, "environment `a` already exists");
}

#[test]
fn retroarch_is_refused_per_game_until_it_is_supported() {
    let (home, paths, roms) = home();
    touch(&roms.join("A.gba"));
    let listing = list(&paths, Emulator::Mgba, &roms);
    let retroarch = picked(home.path(), Emulator::RetroArch);
    let batch = plan_batch(&paths, &retroarch, listing.entries, None).unwrap();
    assert!(batch.ready.is_empty());
    assert_eq!(
        batch.failures[0].reason,
        "RetroArch games aren't supported yet."
    );
}

#[test]
fn a_portable_root_is_refused_in_listing_planning_and_applying() {
    let (home, paths, roms) = home();
    touch(&roms.join("Game.gba"));
    let mgba = picked(home.path(), Emulator::Mgba);
    let listing = list(&paths, Emulator::Mgba, &roms);
    let batch = plan_batch(&paths, &mgba, listing.entries.clone(), None).unwrap();
    let portable = portable_paths(home.path());
    let refused = "Emulated games can't be imported into a portable folder.";
    let error = super::listing(&portable, &Scan::BuiltIn(Emulator::Mgba), &roms).unwrap_err();
    assert_eq!(error.to_string(), refused);
    let error = plan_batch(&portable, &mgba, listing.entries, None).unwrap_err();
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
