use super::*;

#[test]
fn built_in_ids_and_names_are_fixed() {
    assert_eq!(
        Emulator::BUILT_IN.map(Emulator::id),
        [
            "dolphin",
            "retroarch",
            "cemu",
            "pcsx2",
            "duckstation",
            "ppsspp",
            "rpcs3",
            "melonds",
            "mgba",
            "xemu",
            "azahar",
        ]
    );
    assert_eq!(
        Emulator::BUILT_IN.map(Emulator::name),
        [
            "Dolphin",
            "RetroArch",
            "Cemu",
            "PCSX2",
            "DuckStation",
            "PPSSPP",
            "RPCS3",
            "melonDS",
            "mGBA",
            "xemu",
            "Azahar",
        ]
    );
}

#[test]
fn a_config_id_reads_back_and_anything_else_is_other() {
    for emulator in Emulator::BUILT_IN {
        assert_eq!(Emulator::from_config(emulator.id()), emulator);
    }
    for id in [CUSTOM, "eden", "Dolphin", "", "emulated"] {
        assert_eq!(Emulator::from_config(id), Emulator::Other, "{id}");
    }
    assert_eq!(Emulator::Other.name(), "Emulated");
}

fn flatpak_args(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

#[test]
fn the_flatpak_app_is_the_first_word_after_run() {
    let run = flatpak_args(&[
        "run",
        "--filesystem=/games/ps2",
        "net.pcsx2.PCSX2",
        "-batch",
        "--",
        "/games/ps2/a.iso",
    ]);
    assert_eq!(flatpak_app("flatpak", &run), Some("net.pcsx2.PCSX2"));
    assert_eq!(
        flatpak_app("/usr/bin/flatpak", &run),
        Some("net.pcsx2.PCSX2")
    );
    assert_eq!(flatpak_app("dolphin-emu", &run), None);
    assert_eq!(flatpak_app("flatpak", &flatpak_args(&["info", "x"])), None);
    assert_eq!(
        flatpak_app("flatpak", &flatpak_args(&["run", "--verbose"])),
        None
    );
    assert_eq!(flatpak_app("flatpak", &[]), None);
}

#[test]
fn a_flatpak_app_is_found_past_options_with_separate_values() {
    let app = |args: &[&str]| flatpak_app("flatpak", &flatpak_args(args)).map(str::to_owned);
    let id = Some("net.pcsx2.PCSX2".to_owned());
    for args in [
        &["run", "--branch", "stable", "net.pcsx2.PCSX2"][..],
        &["run", "--arch", "x86_64", "net.pcsx2.PCSX2"],
        &["run", "--command", "sh", "net.pcsx2.PCSX2"],
        &["run", "--runtime", "org.x.Platform", "net.pcsx2.PCSX2"],
        &["run", "--commit", "abc", "net.pcsx2.PCSX2"],
        &["run", "--cwd", "/tmp", "net.pcsx2.PCSX2"],
        &["run", "--filesystem", "/roms", "net.pcsx2.PCSX2"],
        // A value shaped like an app id must still be skipped.
        &["run", "--nofilesystem", "xdg.config.dir", "net.pcsx2.PCSX2"],
        &["run", "--parent-pid", "1.2.3", "net.pcsx2.PCSX2"],
        &["run", "--filesystem=/x", "net.pcsx2.PCSX2", "/x/game.iso"],
        &["run", "--", "net.pcsx2.PCSX2"],
        &["run", "net.pcsx2.PCSX2//stable"],
        &["run", "net.pcsx2.PCSX2/x86_64/stable", "-batch"],
        &["--user", "run", "net.pcsx2.PCSX2"],
        &["-v", "run", "net.pcsx2.PCSX2"],
        &["--installation", "games", "run", "net.pcsx2.PCSX2"],
        &["--installation=games", "run", "net.pcsx2.PCSX2"],
    ] {
        assert_eq!(app(args), id, "{args:?}");
    }
}

#[test]
fn a_flatpak_app_that_isnt_an_app_id_gives_nothing() {
    let app = |args: &[&str]| flatpak_app("flatpak", &flatpak_args(args)).map(str::to_owned);
    for args in [
        &["run", "/roms/game.iso"][..],
        &["run", "game.iso"],
        &["run", "--filesystem", "/roms"],
        &["run", "--branch", "stable"],
        &["run", "stable", "net.pcsx2.PCSX2"],
        &["run", "/abs/path.with.dots"],
        &["run", "../a.b.c"],
        &["run", "a..c"],
        &["run", "a.b.c d"],
        &["run", "//stable"],
        &["--user", "info", "net.pcsx2.PCSX2"],
        &["info", "run", "net.pcsx2.PCSX2"],
        &["--installation"],
    ] {
        assert_eq!(app(args), None, "{args:?}");
    }
}

#[test]
fn an_app_counts_as_installed_only_while_its_script_is_there() {
    let dir = tempfile::tempdir().unwrap();
    let system = dir.path().join("system");
    let user = dir.path().join("user");
    std::fs::create_dir_all(&system).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    let exports = [system.clone(), user.clone()];
    assert!(!flatpak_installed(&exports, "org.DolphinEmu.dolphin-emu"));
    std::fs::write(user.join("org.DolphinEmu.dolphin-emu"), "").unwrap();
    assert!(flatpak_installed(&exports, "org.DolphinEmu.dolphin-emu"));
    std::os::unix::fs::symlink(dir.path().join("gone"), system.join("net.pcsx2.PCSX2")).unwrap();
    assert!(!flatpak_installed(&exports, "net.pcsx2.PCSX2"));
    // A path never counts, even when it exists.
    std::fs::write(dir.path().join("a.b.c"), "").unwrap();
    assert!(!flatpak_installed(&exports, "../a.b.c"));
    assert!(!flatpak_installed(&exports, "/tmp"));
}

mod emulation_table {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::test_script::write_script;

    const ROM: &str = "/games/Some Game.iso";

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    fn native(emulator: Emulator) -> Install {
        Install::Path {
            name: emulator.path_names()[0],
            found: PathBuf::from("/usr/bin").join(emulator.path_names()[0]),
        }
    }

    fn flatpak() -> Install {
        Install::Flatpak {
            script: PathBuf::from("/var/lib/flatpak/exports/bin/app"),
        }
    }

    fn build(emulator: Emulator, rom: &str, fullscreen: bool) -> Command {
        build_target(emulator, &native(emulator), Path::new(rom), fullscreen).unwrap()
    }

    fn message(result: Result<Command, crate::error::Error>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn the_table_holds_each_emulators_names_and_flathub_id() {
        let expected: [(Emulator, &[&str], &str); 11] = [
            (
                Emulator::Dolphin,
                &["dolphin-emu"],
                "org.DolphinEmu.dolphin-emu",
            ),
            (
                Emulator::RetroArch,
                &["retroarch"],
                "org.libretro.RetroArch",
            ),
            (Emulator::Cemu, &["cemu", "Cemu"], "info.cemu.Cemu"),
            (Emulator::Pcsx2, &["pcsx2-qt", "pcsx2"], "net.pcsx2.PCSX2"),
            (
                Emulator::DuckStation,
                &["duckstation-qt", "duckstation-nogui"],
                "org.duckstation.DuckStation",
            ),
            (
                Emulator::Ppsspp,
                &["ppsspp", "PPSSPPSDL", "PPSSPPQt"],
                "org.ppsspp.PPSSPP",
            ),
            (Emulator::Rpcs3, &["rpcs3"], "net.rpcs3.RPCS3"),
            (
                Emulator::MelonDs,
                &["melonDS", "melonds"],
                "net.kuribo64.melonDS",
            ),
            (Emulator::Mgba, &["mgba-qt", "mgba"], "io.mgba.mGBA"),
            (Emulator::Xemu, &["xemu"], "app.xemu.xemu"),
            (Emulator::Azahar, &["azahar"], "org.azahar_emu.Azahar"),
        ];
        assert_eq!(expected.map(|(emulator, ..)| emulator), Emulator::BUILT_IN);
        for (emulator, names, app) in expected {
            assert_eq!(emulator.path_names(), names, "{emulator:?}");
            assert_eq!(emulator.flatpak_id(), Some(app), "{emulator:?}");
        }
        assert!(Emulator::Other.path_names().is_empty());
        assert_eq!(Emulator::Other.flatpak_id(), None);
    }

    #[test]
    fn the_table_holds_each_emulators_systems_and_fullscreen_flag() {
        let systems: [(Emulator, &[&str]); 11] = [
            (Emulator::Dolphin, &["gamecube", "wii"]),
            (Emulator::RetroArch, &[]),
            (Emulator::Cemu, &["wiiu"]),
            (Emulator::Pcsx2, &["ps2"]),
            (Emulator::DuckStation, &["ps1"]),
            (Emulator::Ppsspp, &["psp"]),
            (Emulator::Rpcs3, &["ps3"]),
            (Emulator::MelonDs, &["nds"]),
            (Emulator::Mgba, &["gb", "gbc", "gba"]),
            (Emulator::Xemu, &["xbox"]),
            (Emulator::Azahar, &["3ds"]),
        ];
        for (emulator, expected) in systems {
            assert_eq!(emulator.systems(), expected, "{emulator:?}");
            assert_eq!(
                emulator.has_fullscreen_flag(),
                emulator != Emulator::Dolphin,
                "{emulator:?}"
            );
        }
        assert!(Emulator::Other.systems().is_empty());
        assert!(!Emulator::Other.has_fullscreen_flag());
    }

    #[test]
    fn every_extension_tells_a_system_its_emulator_plays() {
        for emulator in Emulator::BUILT_IN {
            for (extension, system) in emulator.extensions() {
                assert_eq!(*extension, extension.to_ascii_lowercase(), "{emulator:?}");
                if let ExtSystem::Is(system) = system {
                    assert!(
                        emulator.systems().contains(system),
                        "{emulator:?} {extension}"
                    );
                }
            }
        }
    }

    #[test]
    fn each_emulators_extensions_map_to_their_systems() {
        use ExtSystem::{Ambiguous, Is};
        let expected: [(Emulator, Vec<(&str, ExtSystem)>); 10] = [
            (
                Emulator::Dolphin,
                vec![
                    ("gcm", Is("gamecube")),
                    ("tgc", Is("gamecube")),
                    ("wbfs", Is("wii")),
                    ("wad", Is("wii")),
                    ("iso", Ambiguous),
                    ("ciso", Ambiguous),
                    ("gcz", Ambiguous),
                    ("wia", Ambiguous),
                    ("rvz", Ambiguous),
                    ("elf", Ambiguous),
                    ("dol", Ambiguous),
                ],
            ),
            (
                Emulator::Cemu,
                ["wud", "wux", "wua", "wuhb"]
                    .map(|extension| (extension, Is("wiiu")))
                    .to_vec(),
            ),
            (
                Emulator::Pcsx2,
                ["iso", "bin", "img", "mdf", "chd", "cso", "zso", "gz"]
                    .map(|extension| (extension, Is("ps2")))
                    .to_vec(),
            ),
            (
                Emulator::DuckStation,
                [
                    "cue", "chd", "iso", "pbp", "ecm", "mds", "ccd", "m3u", "bin", "img",
                ]
                .map(|extension| (extension, Is("ps1")))
                .to_vec(),
            ),
            (
                Emulator::Ppsspp,
                ["iso", "cso", "chd", "pbp"]
                    .map(|extension| (extension, Is("psp")))
                    .to_vec(),
            ),
            (Emulator::Rpcs3, vec![("iso", Is("ps3"))]),
            (
                Emulator::MelonDs,
                ["nds", "srl", "dsi", "ids"]
                    .map(|extension| (extension, Is("nds")))
                    .to_vec(),
            ),
            (
                Emulator::Mgba,
                vec![
                    ("gb", Is("gb")),
                    ("sgb", Is("gb")),
                    ("gbc", Is("gbc")),
                    ("gba", Is("gba")),
                    ("agb", Is("gba")),
                    ("mb", Is("gba")),
                ],
            ),
            (Emulator::Xemu, vec![("iso", Is("xbox"))]),
            (
                Emulator::Azahar,
                [
                    "cci", "3ds", "cia", "3dsx", "cxi", "app", "zcci", "zcia", "z3dsx", "zcxi",
                ]
                .map(|extension| (extension, Is("3ds")))
                .to_vec(),
            ),
        ];
        for (emulator, extensions) in expected {
            assert_eq!(emulator.extensions(), extensions, "{emulator:?}");
        }
        assert!(Emulator::RetroArch.extensions().is_empty());
        assert!(Emulator::Other.extensions().is_empty());
    }

    #[test]
    fn extensions_compare_without_case_and_unlisted_ones_tell_nothing() {
        assert_eq!(
            Emulator::Mgba.extension_system("GBA"),
            Some(ExtSystem::Is("gba"))
        );
        assert_eq!(
            Emulator::Dolphin.extension_system("RvZ"),
            Some(ExtSystem::Ambiguous)
        );
        assert_eq!(Emulator::Pcsx2.extension_system("cue"), None);
        assert_eq!(Emulator::Mgba.extension_system("rom"), None);
        assert_eq!(Emulator::Mgba.extension_system(".gba"), None);
        assert_eq!(Emulator::RetroArch.extension_system("gba"), None);
    }

    #[test]
    fn folder_games_are_recorded_for_the_three_that_take_them() {
        for emulator in Emulator::BUILT_IN {
            let expected = match emulator {
                Emulator::Cemu => Some(FolderGame::CemuTitle),
                Emulator::Ppsspp => Some(FolderGame::PpssppEboot),
                Emulator::Rpcs3 => Some(FolderGame::Rpcs3Disc),
                _ => None,
            };
            assert_eq!(emulator.folder_game(), expected, "{emulator:?}");
        }
        assert_eq!(Emulator::Other.folder_game(), None);
    }

    #[test]
    fn each_command_line_matches_the_table_with_and_without_fullscreen() {
        let rom = ROM;
        let cases: [(Emulator, Vec<&str>, Vec<&str>); 10] = [
            (
                Emulator::Dolphin,
                vec!["-b", "-e", rom],
                vec!["-b", "-e", rom],
            ),
            (Emulator::Cemu, vec!["-g", rom], vec!["-f", "-g", rom]),
            (
                Emulator::Pcsx2,
                vec!["-batch", "--", rom],
                vec!["-batch", "-fullscreen", "--", rom],
            ),
            (
                Emulator::DuckStation,
                vec!["-batch", "--", rom],
                vec!["-batch", "-fullscreen", "--", rom],
            ),
            (
                Emulator::Ppsspp,
                vec!["--pause-menu-exit", rom],
                vec!["--pause-menu-exit", "--fullscreen", rom],
            ),
            (
                Emulator::Rpcs3,
                vec!["--no-gui", rom],
                vec!["--no-gui", "--fullscreen", rom],
            ),
            (Emulator::MelonDs, vec![rom], vec!["-f", rom]),
            (Emulator::Mgba, vec![rom], vec!["-f", rom]),
            (
                Emulator::Xemu,
                vec!["-dvd_path", rom],
                vec!["-full-screen", "-dvd_path", rom],
            ),
            (Emulator::Azahar, vec![rom], vec!["-f", rom]),
        ];
        for (emulator, plain, full) in cases {
            let off = build(emulator, rom, false);
            let on = build(emulator, rom, true);
            assert_eq!(off.exe, emulator.path_names()[0], "{emulator:?}");
            assert_eq!(on.exe, off.exe, "{emulator:?}");
            assert_eq!(off.args, strings(&plain), "{emulator:?} windowed");
            assert_eq!(on.args, strings(&full), "{emulator:?} fullscreen");
        }
    }

    #[test]
    fn a_native_target_keeps_the_name_it_was_found_by() {
        let install = Install::Path {
            name: "PPSSPPSDL",
            found: PathBuf::from("/opt/bin/PPSSPPSDL"),
        };
        let command = build_target(Emulator::Ppsspp, &install, Path::new(ROM), false).unwrap();
        assert_eq!(command.exe, "PPSSPPSDL");
    }

    #[test]
    fn a_picked_file_is_the_exe_for_a_built_in_with_its_own_arguments() {
        let install = Install::File(PathBuf::from("/home/me/Apps/Cemu-2.0.AppImage"));
        let command = build_target(Emulator::Cemu, &install, Path::new(ROM), true).unwrap();
        assert_eq!(command.exe, "/home/me/Apps/Cemu-2.0.AppImage");
        assert_eq!(command.args, strings(&["-f", "-g", ROM]));
    }

    #[test]
    fn a_flatpak_target_runs_the_app_with_the_roms_folder_read_write() {
        let command = build_target(Emulator::Pcsx2, &flatpak(), Path::new(ROM), true).unwrap();
        assert_eq!(command.exe, "flatpak");
        assert_eq!(
            command.args,
            strings(&[
                "run",
                "--filesystem=/games",
                "net.pcsx2.PCSX2",
                "-batch",
                "-fullscreen",
                "--",
                ROM,
            ])
        );
        assert!(!command.args.iter().any(|arg| arg.contains(":ro")));
    }

    #[test]
    fn every_flatpak_target_reads_back_to_its_app_id() {
        for emulator in Emulator::BUILT_IN {
            if emulator == Emulator::RetroArch {
                continue;
            }
            let command = build_target(emulator, &flatpak(), Path::new(ROM), true).unwrap();
            assert_eq!(
                flatpak_app(&command.exe, &command.args),
                emulator.flatpak_id(),
                "{emulator:?}"
            );
        }
    }

    #[test]
    fn a_flatpak_target_runs_the_same_emulator_arguments_as_a_native_one() {
        for emulator in Emulator::BUILT_IN {
            if emulator == Emulator::RetroArch {
                continue;
            }
            let native = build(emulator, ROM, true);
            let sandboxed = build_target(emulator, &flatpak(), Path::new(ROM), true).unwrap();
            assert!(
                sandboxed.args.ends_with(&native.args),
                "{emulator:?}: {:?}",
                sandboxed.args
            );
        }
    }

    #[test]
    fn a_colon_and_a_backslash_in_the_folder_are_escaped_for_filesystem() {
        let command = build_target(
            Emulator::Dolphin,
            &flatpak(),
            Path::new("/mnt/a:b\\c/game.rvz"),
            false,
        )
        .unwrap();
        assert_eq!(command.args[1], "--filesystem=/mnt/a\\:b\\\\c");
        // The ROM itself is a plain argument, not escaped.
        assert_eq!(command.args.last().unwrap(), "/mnt/a:b\\c/game.rvz");
    }

    #[test]
    fn a_rom_linked_in_from_another_folder_gets_its_real_folder_too() {
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("library");
        let real = dir.path().join("real:disks");
        std::fs::create_dir_all(&library).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("game.iso"), "").unwrap();
        let rom = library.join("game.iso");
        std::os::unix::fs::symlink(real.join("game.iso"), &rom).unwrap();
        let real = real.canonicalize().unwrap();
        let library = library.canonicalize().unwrap();

        let command = build_target(
            Emulator::Pcsx2,
            &flatpak(),
            &library.join("game.iso"),
            false,
        )
        .unwrap();
        let filesystems: Vec<&String> = command
            .args
            .iter()
            .filter(|arg| arg.starts_with("--filesystem="))
            .collect();
        assert_eq!(
            filesystems,
            [
                &format!("--filesystem={}", library.display()),
                &format!(
                    "--filesystem={}",
                    real.to_str().unwrap().replace(':', "\\:")
                ),
            ]
        );
        assert_eq!(
            flatpak_app(&command.exe, &command.args),
            Some("net.pcsx2.PCSX2")
        );
    }

    #[test]
    fn a_rom_in_its_own_folder_adds_that_folder_once() {
        let dir = tempfile::tempdir().unwrap();
        let rom = dir.path().canonicalize().unwrap().join("game.iso");
        std::fs::write(&rom, "").unwrap();
        let command = build_target(Emulator::Pcsx2, &flatpak(), &rom, false).unwrap();
        let count = command
            .args
            .iter()
            .filter(|arg| arg.starts_with("--filesystem="))
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn a_folder_game_is_passed_as_the_folder_and_that_folder_is_shared() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        for (emulator, flag) in [
            (Emulator::Cemu, "-g"),
            (Emulator::Ppsspp, "--pause-menu-exit"),
            (Emulator::Rpcs3, "--no-gui"),
        ] {
            let game = dir.join(format!("{emulator:?} title"));
            std::fs::create_dir_all(game.join("code")).unwrap();
            let native = build_target(emulator, &native(emulator), &game, false).unwrap();
            assert_eq!(native.args, strings(&[flag, game.to_str().unwrap()]));
            let sandboxed = build_target(emulator, &flatpak(), &game, false).unwrap();
            assert_eq!(
                sandboxed.args[1],
                format!("--filesystem={}", game.display()),
                "{emulator:?}"
            );
            assert_eq!(sandboxed.args[2], emulator.flatpak_id().unwrap());
            assert_eq!(sandboxed.args.last().unwrap(), game.to_str().unwrap());
        }
    }

    #[test]
    fn a_folder_game_linked_from_elsewhere_gets_its_real_folder_too() {
        let dir = tempfile::tempdir().unwrap();
        let dir = dir.path().canonicalize().unwrap();
        let real = dir.join("real/Title");
        std::fs::create_dir_all(real.join("code")).unwrap();
        std::fs::create_dir_all(dir.join("library")).unwrap();
        let link = dir.join("library/Title");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let command = build_target(Emulator::Cemu, &flatpak(), &link, false).unwrap();
        assert_eq!(command.args[1], format!("--filesystem={}", link.display()));
        assert_eq!(command.args[2], format!("--filesystem={}", real.display()));
    }

    #[test]
    fn paths_with_spaces_and_odd_characters_stay_single_arguments() {
        for rom in [
            "/games/My Game (USA) [!].iso",
            "/games/it's \"quoted\".iso",
            "/games/$HOME `id` $(id) *.iso",
            "/games/\u{30b2}\u{30fc}\u{30e0}/caf\u{e9} \u{1f3ae}.iso",
            "/games/-dash.iso",
            "/games/semi;colon&pipe|.iso",
        ] {
            for emulator in Emulator::BUILT_IN {
                if emulator == Emulator::RetroArch {
                    continue;
                }
                let command = build(emulator, rom, true);
                assert_eq!(command.args.last().unwrap(), rom, "{emulator:?} {rom}");
                let whole = command.args.iter().filter(|arg| *arg == rom).count();
                assert_eq!(whole, 1, "{emulator:?} {rom}");
            }
            let sandboxed =
                build_target(Emulator::Pcsx2, &flatpak(), Path::new(rom), true).unwrap();
            let folder = Path::new(rom).parent().unwrap().to_str().unwrap();
            assert_eq!(sandboxed.args[1], format!("--filesystem={folder}"));
            assert_eq!(sandboxed.args.last().unwrap(), rom);
            assert_eq!(sandboxed.args.len(), 7, "{rom}");
        }
    }

    #[test]
    fn a_path_that_is_not_utf8_is_refused() {
        use std::os::unix::ffi::OsStrExt;
        let rom = Path::new(std::ffi::OsStr::from_bytes(b"/games/\xff.iso"));
        let error = build_target(Emulator::Mgba, &native(Emulator::Mgba), rom, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("isn't valid UTF-8"), "{error}");
        assert!(build_target(Emulator::Mgba, &flatpak(), rom, false).is_err());
        assert!(custom_target("emu", "{rom}", rom).is_err());
    }

    #[test]
    fn a_relative_rom_is_refused_so_it_never_starts_with_a_dash() {
        for rom in [
            "game.iso",
            "-b",
            "./game.iso",
            "games/game.iso",
            "~/game.iso",
            "",
        ] {
            for emulator in Emulator::BUILT_IN {
                let error = message(build_target(
                    emulator,
                    &native(emulator),
                    Path::new(rom),
                    false,
                ));
                assert!(
                    error.contains("isn't absolute"),
                    "{emulator:?} {rom}: {error}"
                );
            }
            assert!(build_target(Emulator::Pcsx2, &flatpak(), Path::new(rom), false).is_err());
            assert!(custom_target("emu", "{rom}", Path::new(rom)).is_err());
        }
    }

    #[test]
    fn retroarch_is_refused_for_now_native_or_flatpak() {
        for install in [native(Emulator::RetroArch), flatpak()] {
            let error = message(build_target(
                Emulator::RetroArch,
                &install,
                Path::new(ROM),
                true,
            ));
            assert_eq!(error, "RetroArch games aren't supported yet.");
        }
    }

    #[test]
    fn a_custom_emulator_has_no_built_in_command_line() {
        let error = message(build_target(
            Emulator::Other,
            &Install::File(PathBuf::from("/bin/emu")),
            Path::new(ROM),
            false,
        ));
        assert_eq!(error, "A custom emulator has no built-in command line.");
    }

    fn fake_roots() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("bin1");
        let second = dir.path().join("bin2");
        let system = dir.path().join("system-exports");
        let user = dir.path().join("user-exports");
        for folder in [&first, &second, &system, &user] {
            std::fs::create_dir_all(folder).unwrap();
        }
        (dir, first, second, system, user)
    }

    fn path_of(folders: &[&Path]) -> std::ffi::OsString {
        std::env::join_paths(folders).unwrap()
    }

    #[test]
    fn installs_come_from_path_in_table_order_with_the_first_hit_per_name() {
        let (_dir, first, second, system, _user) = fake_roots();
        // The table lists pcsx2-qt before pcsx2, whatever the folder order.
        write_script(second.join("pcsx2-qt"), "#!/bin/sh\n");
        write_script(first.join("pcsx2"), "#!/bin/sh\n");
        write_script(second.join("pcsx2"), "#!/bin/sh\n");
        let found = installs(Emulator::Pcsx2, &path_of(&[&first, &second]), &[system]);
        assert_eq!(
            found,
            [
                Install::Path {
                    name: "pcsx2-qt",
                    found: second.join("pcsx2-qt"),
                },
                Install::Path {
                    name: "pcsx2",
                    found: first.join("pcsx2"),
                },
            ]
        );
    }

    #[test]
    fn installs_skip_what_is_not_an_executable_file() {
        let (_dir, first, second, _system, _user) = fake_roots();
        std::fs::write(first.join("pcsx2-qt"), "").unwrap();
        std::fs::create_dir(first.join("pcsx2")).unwrap();
        write_script(second.join("pcsx2"), "#!/bin/sh\n");
        let path = path_of(&[&first, &second]);
        let found = installs(Emulator::Pcsx2, &path, &[]);
        assert_eq!(
            found,
            [Install::Path {
                name: "pcsx2",
                found: second.join("pcsx2"),
            }]
        );
    }

    #[test]
    fn installs_list_the_flatpak_after_path_and_prefer_the_system_export() {
        let (_dir, first, _second, system, user) = fake_roots();
        write_script(first.join("net.pcsx2.PCSX2"), "#!/bin/sh\n");
        write_script(first.join("pcsx2-qt"), "#!/bin/sh\n");
        write_script(system.join("net.pcsx2.PCSX2"), "#!/bin/sh\n");
        write_script(user.join("net.pcsx2.PCSX2"), "#!/bin/sh\n");
        let exports = [system.clone(), user.clone()];
        let found = installs(Emulator::Pcsx2, &path_of(&[&first]), &exports);
        assert_eq!(
            found,
            [
                Install::Path {
                    name: "pcsx2-qt",
                    found: first.join("pcsx2-qt"),
                },
                Install::Flatpak {
                    script: system.join("net.pcsx2.PCSX2"),
                },
            ]
        );
        // With only the user's export, that one counts.
        std::fs::remove_file(system.join("net.pcsx2.PCSX2")).unwrap();
        let found = installs(Emulator::Pcsx2, &path_of(&[]), &exports);
        assert_eq!(
            found,
            [Install::Flatpak {
                script: user.join("net.pcsx2.PCSX2"),
            }]
        );
    }

    #[test]
    fn an_export_script_that_is_a_dangling_link_is_not_an_install() {
        let (dir, _first, _second, system, user) = fake_roots();
        std::os::unix::fs::symlink(dir.path().join("gone"), system.join("net.pcsx2.PCSX2"))
            .unwrap();
        let exports = [system.clone(), user.clone()];
        assert!(installs(Emulator::Pcsx2, &path_of(&[]), &exports).is_empty());
        // A good script further down still counts.
        write_script(user.join("net.pcsx2.PCSX2"), "#!/bin/sh\n");
        assert_eq!(
            installs(Emulator::Pcsx2, &path_of(&[]), &exports),
            [Install::Flatpak {
                script: user.join("net.pcsx2.PCSX2"),
            }]
        );
    }

    #[test]
    fn installs_look_only_for_the_asked_emulator_and_retroarch_is_found_like_the_rest() {
        let (_dir, first, _second, system, _user) = fake_roots();
        write_script(first.join("retroarch"), "#!/bin/sh\n");
        write_script(system.join("org.libretro.RetroArch"), "#!/bin/sh\n");
        let path = path_of(&[&first]);
        let exports = [system.clone()];
        assert_eq!(installs(Emulator::RetroArch, &path, &exports).len(), 2);
        assert!(installs(Emulator::Pcsx2, &path, &exports).is_empty());
        assert!(installs(Emulator::Other, &path, &exports).is_empty());
    }

    #[test]
    fn a_picked_file_must_be_an_absolute_executable_file() {
        let (dir, first, ..) = fake_roots();
        let good = first.join("Cemu.AppImage");
        write_script(&good, "#!/bin/sh\n");
        assert_eq!(pick_file(&good).unwrap(), Install::File(good.clone()));

        let plain = first.join("plain");
        std::fs::write(&plain, "").unwrap();
        let error = pick_file(&plain).unwrap_err();
        assert!(
            matches!(error, crate::error::Error::NotExecutable(_)),
            "{error}"
        );

        let error = pick_file(&dir.path().join("missing")).unwrap_err();
        assert!(
            matches!(error, crate::error::Error::ExeNotFound(_)),
            "{error}"
        );
        // A folder has the execute bit, but isn't a file.
        let error = pick_file(&first).unwrap_err();
        assert!(
            matches!(error, crate::error::Error::ExeNotFound(_)),
            "{error}"
        );
        let error = pick_file(Path::new("Cemu.AppImage")).unwrap_err();
        assert!(
            matches!(error, crate::error::Error::RelativeExe(_)),
            "{error}"
        );
    }

    #[test]
    fn a_custom_target_fills_in_rom_as_a_whole_argument() {
        let command = custom_target("my-emu", "--fast {rom}", Path::new(ROM)).unwrap();
        assert_eq!(command.exe, "my-emu");
        assert_eq!(command.args, strings(&["--fast", ROM]));
    }

    #[test]
    fn a_custom_target_fills_in_rom_inside_an_argument_and_everywhere_it_appears() {
        let command = custom_target(
            "/opt/emu/run",
            "--rom={rom} -- {rom} --log={rom}.log",
            Path::new(ROM),
        )
        .unwrap();
        assert_eq!(command.exe, "/opt/emu/run");
        assert_eq!(
            command.args,
            strings(&[
                &format!("--rom={ROM}"),
                "--",
                ROM,
                &format!("--log={ROM}.log"),
            ])
        );
    }

    #[test]
    fn a_custom_rom_path_is_not_split_or_filled_in_again() {
        let rom = "/games/a {rom} b.iso";
        let command = custom_target("emu", "-x {rom}", Path::new(rom)).unwrap();
        assert_eq!(command.args, strings(&["-x", rom]));
    }

    #[test]
    fn a_custom_target_needs_rom_and_a_closed_quote() {
        let error = message(custom_target("emu", "--fast --windowed", Path::new(ROM)));
        assert_eq!(error, "The arguments need {rom} for the game.");
        let error = message(custom_target("emu", "", Path::new(ROM)));
        assert_eq!(error, "The arguments need {rom} for the game.");
        let error = message(custom_target(
            "emu",
            "--title 'Unclosed {rom}",
            Path::new(ROM),
        ));
        assert_eq!(error, "The arguments have an unclosed quote.");
    }

    #[test]
    fn quotes_in_custom_arguments_keep_spaces_together() {
        let command = custom_target(
            "emu",
            r#"--title "My Game" --opt='a b' --dir=\ x {rom}"#,
            Path::new(ROM),
        )
        .unwrap();
        assert_eq!(
            command.args,
            strings(&["--title", "My Game", "--opt=a b", "--dir= x", ROM])
        );
        // A quoted placeholder still counts.
        let command = custom_target("emu", r#""{rom}""#, Path::new(ROM)).unwrap();
        assert_eq!(command.args, strings(&[ROM]));
    }
}
