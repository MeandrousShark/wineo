use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};

use tempfile::{TempDir, tempdir};

use super::*;
use crate::emulator::retroarch_target;

fn paths(home: &Path) -> Paths {
    Paths::from_lookup(|key| (key == "HOME").then(|| home.as_os_str().to_owned())).unwrap()
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn info(display_name: &str, systemid: &str, extensions: &str, database: &str) -> String {
    format!(
        "display_name = \"{display_name}\"\nsystemid = \"{systemid}\"\n\
         supported_extensions = \"{extensions}\"\ndatabase = \"{database}\"\n"
    )
}

/// A home folder and a stand-in for `/`, so nothing is read from the host.
struct Fixture {
    home: TempDir,
    root: TempDir,
    paths: Paths,
}

impl Fixture {
    fn new() -> Self {
        let home = tempdir().unwrap();
        let root = tempdir().unwrap();
        let paths = paths(home.path());
        Self { home, root, paths }
    }

    fn native(&self) -> CoreFolders {
        let install = Install::Path {
            name: "retroarch",
            found: PathBuf::from("/usr/bin/retroarch"),
        };
        core_folders(&self.paths, &install, self.root.path())
    }

    fn flatpak(&self) -> CoreFolders {
        let install = Install::Flatpak {
            script: PathBuf::from("/var/lib/flatpak/exports/bin/org.libretro.RetroArch"),
        };
        core_folders(&self.paths, &install, self.root.path())
    }

    fn system(&self, path: &str) -> PathBuf {
        self.root.path().join(path)
    }

    fn home(&self, path: &str) -> PathBuf {
        self.home.path().join(path)
    }
}

fn found(cores: &[Core]) -> Vec<(&str, Option<&'static str>, &Path)> {
    cores
        .iter()
        .map(|core| (core.name(), core.system(), core.path.as_path()))
        .collect()
}

#[test]
fn info_files_are_read_with_quotes_odd_spacing_and_missing_keys() {
    let parsed = parse_info(concat!(
        "# a comment = \"no\"\n",
        "display_name = \"Nintendo - SNES / SFC (Snes9x - Current)\"\n",
        "systemid=super_nes\n",
        "  supported_extensions   =   \"smc|SFC| |.swc|\"  \n",
        "database = \"Nintendo - Super Nintendo Entertainment System|Nintendo - Satellaview\"\n",
        "corename = \"Snes9x\"\n",
        "not a key line\n",
        "license = \"Non-commercial\"\n",
    ));
    assert_eq!(
        parsed,
        CoreInfo {
            display_name: Some("Nintendo - SNES / SFC (Snes9x - Current)".to_owned()),
            systemid: Some("super_nes".to_owned()),
            extensions: vec!["smc".to_owned(), "sfc".to_owned(), "swc".to_owned()],
            database: vec![
                "Nintendo - Super Nintendo Entertainment System".to_owned(),
                "Nintendo - Satellaview".to_owned(),
            ],
        }
    );
    assert_eq!(parse_info(""), CoreInfo::default());
    assert_eq!(
        parse_info("display_name = \"\"\nsystemid = \"\nsupported_extensions = \"\"\n"),
        CoreInfo::default()
    );
    // An unclosed quote keeps what follows it.
    assert_eq!(
        parse_info("display_name = \"Half").display_name.as_deref(),
        Some("Half")
    );
}

#[test]
fn a_native_retroarch_finds_distro_and_downloaded_cores_with_their_info() {
    let f = Fixture::new();
    // Arch and Debian put info apart from the cores.
    write(&f.system("usr/lib/libretro/snes9x_libretro.so"), "");
    write(
        &f.system("usr/share/libretro/info/snes9x_libretro.info"),
        &info("Snes9x", "super_nes", "smc|sfc", "Nintendo - SNES"),
    );
    write(
        &f.system("usr/lib/x86_64-linux-gnu/libretro/gambatte_libretro.so"),
        "",
    );
    write(&f.system("usr/lib64/libretro/nestopia_libretro.so"), "");
    // A downloader keeps info beside the cores, for every core it knows.
    let user = f.home(".config/retroarch/cores");
    write(&user.join("mgba_libretro.so"), "");
    write(
        &user.join("mgba_libretro.info"),
        &info("mGBA", "game_boy_advance", "gb|gbc|gba", "GB|GBC|GBA"),
    );
    write(
        &user.join("gambatte_libretro.info"),
        &info("Gambatte", "game_boy", "gb|gbc", "GB|GBC"),
    );
    write(
        &user.join("melondsds_libretro.info"),
        &info("melonDS DS", "nds", "nds", "NDS"),
    );
    write(&user.join("core_info.cache"), "");
    write(&user.join("not_a_core.so"), "");
    fs::create_dir_all(user.join("folder_libretro.so")).unwrap();

    let cores = cores(&f.native());
    assert_eq!(
        found(&cores),
        [
            // Its info is only in the downloader's folder, which isn't its own.
            (
                "gambatte_libretro.so",
                None,
                f.system("usr/lib/x86_64-linux-gnu/libretro/gambatte_libretro.so")
                    .as_path()
            ),
            ("mGBA", Some("gba"), user.join("mgba_libretro.so").as_path()),
            (
                "nestopia_libretro.so",
                None,
                f.system("usr/lib64/libretro/nestopia_libretro.so")
                    .as_path()
            ),
            (
                "Snes9x",
                Some("snes"),
                f.system("usr/lib/libretro/snes9x_libretro.so").as_path()
            ),
        ]
    );
    assert_eq!(cores[0].id(), "gambatte");
    assert_eq!(cores[3].file_name(), "snes9x_libretro.so");
}

#[test]
fn a_core_linked_from_two_folders_counts_once() {
    let f = Fixture::new();
    write(&f.system("usr/lib/libretro/snes9x_libretro.so"), "");
    symlink(f.system("usr/lib"), f.system("usr/lib64")).unwrap();
    let cores = cores(&f.native());
    assert_eq!(cores.len(), 1);
    assert_eq!(
        cores[0].path,
        f.system("usr/lib/libretro/snes9x_libretro.so")
    );
}

#[test]
fn the_flatpak_finds_its_own_cores_with_info_from_inside_the_app() {
    let f = Fixture::new();
    let cores_dir = f.home(".var/app/org.libretro.RetroArch/config/retroarch/cores");
    write(&cores_dir.join("mgba_libretro.so"), "");
    write(&cores_dir.join("snes9x_libretro.so"), "");
    write(&cores_dir.join("bare_libretro.so"), "");
    write(
        &f.system(
            "var/lib/flatpak/app/org.libretro.RetroArch/current/active/files/share/libretro/info/mgba_libretro.info",
        ),
        &info("mGBA", "game_boy_advance", "gb|gbc|gba", "GB|GBC|GBA"),
    );
    // A user install of the app.
    write(
        &f.home(
            ".local/share/flatpak/app/org.libretro.RetroArch/current/active/files/share/libretro/info/snes9x_libretro.info",
        ),
        &info("Snes9x", "super_nes", "smc|sfc", "SNES"),
    );
    let cores = cores(&f.flatpak());
    assert_eq!(
        found(&cores),
        [
            (
                "bare_libretro.so",
                None,
                cores_dir.join("bare_libretro.so").as_path()
            ),
            (
                "mGBA",
                Some("gba"),
                cores_dir.join("mgba_libretro.so").as_path()
            ),
            (
                "Snes9x",
                Some("snes"),
                cores_dir.join("snes9x_libretro.so").as_path()
            ),
        ]
    );
}

#[test]
fn distro_cores_are_never_offered_for_the_flatpak_or_the_reverse() {
    let f = Fixture::new();
    write(&f.system("usr/lib/libretro/snes9x_libretro.so"), "");
    write(&f.home(".config/retroarch/cores/mgba_libretro.so"), "");
    assert_eq!(cores(&f.native()).len(), 2);
    assert!(cores(&f.flatpak()).is_empty());

    let f = Fixture::new();
    write(
        &f.home(".var/app/org.libretro.RetroArch/config/retroarch/cores/mgba_libretro.so"),
        "",
    );
    assert_eq!(cores(&f.flatpak()).len(), 1);
    assert!(cores(&f.native()).is_empty());
}

#[test]
fn no_cores_are_found_in_missing_or_empty_folders_or_from_info_alone() {
    let f = Fixture::new();
    assert!(cores(&f.native()).is_empty());
    assert!(cores(&f.flatpak()).is_empty());
    fs::create_dir_all(f.system("usr/lib/libretro")).unwrap();
    write(
        &f.home(".config/retroarch/cores/snes9x_libretro.info"),
        &info("Snes9x", "super_nes", "sfc", "SNES"),
    );
    write(&f.system("usr/share/libretro/info/mgba_libretro.info"), "");
    assert!(cores(&f.native()).is_empty());
}

#[test]
fn systemids_map_to_wineos_systems_and_others_to_none() {
    for (systemid, system) in [
        ("super_nes", "snes"),
        ("mega_drive", "genesis"),
        ("playstation", "ps1"),
        ("playstation2", "ps2"),
        ("playstation_portable", "psp"),
        ("game_boy", "gb"),
        ("game_boy_advance", "gba"),
        ("nintendo_64", "n64"),
        ("n64", "n64"),
        ("gamecube", "gamecube"),
        ("wiiu", "wiiu"),
        ("nds", "nds"),
        ("3ds", "3ds"),
        ("xbox", "xbox"),
        ("master_system", "mastersystem"),
        ("neo_geo_pocket", "ngp"),
        ("neo_geo_pocket_color", "ngpc"),
        ("wonderswan", "wonderswan"),
        ("wonderswan_color", "wonderswancolor"),
    ] {
        assert_eq!(system_for_id(systemid), Some(system), "{systemid}");
    }
    for unknown in ["mame", "dos", "2048", "", "SUPER_NES", "snes"] {
        assert_eq!(system_for_id(unknown), None, "{unknown}");
    }
    // Every system the table gives is a tag-safe id.
    for (_, system) in SYSTEM_IDS
        .iter()
        .chain(EXTENSION_SYSTEMS)
        .chain(DATABASE_NAMES)
    {
        assert_eq!(crate::name::from_title(system), *system);
    }
}

#[test]
fn a_core_whose_system_is_unknown_or_missing_gives_no_system() {
    let core = |text: &str| Core {
        path: PathBuf::from("/cores/x_libretro.so"),
        info: Some(parse_info(text)),
    };
    let mame = core(&info("MAME", "mame", "zip|chd", "MAME"));
    assert_eq!(mame.system(), None);
    assert_eq!(mame.file_system(Some("zip")), None);
    let none = core("display_name = \"Something\"\nsupported_extensions = \"zip\"\n");
    assert_eq!(none.system(), None);
    assert_eq!(none.file_system(Some("zip")), None);
}

#[test]
fn a_core_of_several_systems_tags_each_file_by_its_extension() {
    let core = |text: String| Core {
        path: PathBuf::from("/cores/x_libretro.so"),
        info: Some(parse_info(&text)),
    };
    let mgba = core(info(
        "mGBA",
        "game_boy_advance",
        "gb|gbc|gba",
        "Nintendo - Game Boy|Nintendo - Game Boy Color|Nintendo - Game Boy Advance",
    ));
    let gpgx = core(info(
        "Genesis Plus GX",
        "mega_drive",
        "mdx|md|smd|gen|bin|cue|iso|sms|bms|gg|sg|68k|sgd|chd|m3u",
        "Sega - Game Gear|Sega - Master System - Mark III|Sega - Mega Drive - Genesis",
    ));
    let gambatte = core(info(
        "Gambatte",
        "game_boy",
        "gb|gbc|dmg",
        "Nintendo - Game Boy|Nintendo - Game Boy Color",
    ));
    for (core, extension, system) in [
        (&mgba, "gb", Some("gb")),
        (&mgba, "gbc", Some("gbc")),
        (&mgba, "gba", Some("gba")),
        (&gpgx, "md", Some("genesis")),
        (&gpgx, "gen", Some("genesis")),
        (&gpgx, "smd", Some("genesis")),
        (&gpgx, "sms", Some("mastersystem")),
        (&gpgx, "gg", Some("gamegear")),
        // Too generic to place.
        (&gpgx, "bin", None),
        (&gpgx, "cue", None),
        (&gpgx, "chd", None),
        (&gambatte, "gb", Some("gb")),
        (&gambatte, "gbc", Some("gbc")),
        (&gambatte, "dmg", Some("gb")),
    ] {
        assert_eq!(
            core.file_system(Some(extension)),
            system,
            "{} {extension}",
            core.name()
        );
    }
    assert_eq!(gpgx.file_system(None), None);

    // A core of one system gives it for every file, generic ones too.
    let duckstation = core(info(
        "DuckStation",
        "playstation",
        "cue|bin|chd",
        "Sony - PlayStation",
    ));
    assert_eq!(duckstation.file_system(Some("bin")), Some("ps1"));
    assert_eq!(duckstation.file_system(None), Some("ps1"));
}

#[test]
fn a_core_without_info_goes_by_its_file_name_and_plays_anything() {
    let core = Core {
        path: PathBuf::from("/cores/odd_libretro.so"),
        info: None,
    };
    assert_eq!(core.name(), "odd_libretro.so");
    assert_eq!(core.id(), "odd");
    assert_eq!(core.extensions(), None);
    assert!(core.plays(Some("anything")));
    assert!(core.plays(None));
    assert_eq!(core.system(), None);
    assert_eq!(core.file_system(Some("gba")), None);

    // With info that lists nothing, it plays nothing.
    let gameless = Core {
        info: Some(parse_info("display_name = \"2048\"\nsystemid = \"2048\"\n")),
        ..core
    };
    assert!(!gameless.plays(Some("gba")));
    assert!(!gameless.plays(None));
}

fn snes9x(path: &str) -> Core {
    Core {
        path: PathBuf::from(path),
        info: Some(parse_info(&info("Snes9x", "super_nes", "sfc", "SNES"))),
    }
}

const ROM: &str = "/games/snes/Super Metroid (USA).sfc";
const QUIT: &str = "/home/me/.local/share/wineo/retroarch-quit.cfg";

fn strings(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

#[test]
fn a_native_target_appends_the_quit_file_and_loads_the_cores_full_path() {
    let install = Install::Path {
        name: "retroarch",
        found: PathBuf::from("/usr/bin/retroarch"),
    };
    let core = snes9x("/usr/lib/libretro/snes9x_libretro.so");
    let on = retroarch_target(&install, &core, Path::new(ROM), true, Path::new(QUIT)).unwrap();
    assert_eq!(on.exe, "retroarch");
    assert_eq!(
        on.args,
        strings(&[
            "--appendconfig",
            QUIT,
            "-f",
            "-L",
            "/usr/lib/libretro/snes9x_libretro.so",
            ROM
        ])
    );
    let off = retroarch_target(&install, &core, Path::new(ROM), false, Path::new(QUIT)).unwrap();
    assert_eq!(
        off.args,
        strings(&[
            "--appendconfig",
            QUIT,
            "-L",
            "/usr/lib/libretro/snes9x_libretro.so",
            ROM
        ])
    );
    let picked = Install::File(PathBuf::from("/home/me/Apps/RetroArch.AppImage"));
    let file = retroarch_target(&picked, &core, Path::new(ROM), false, Path::new(QUIT)).unwrap();
    assert_eq!(file.exe, "/home/me/Apps/RetroArch.AppImage");
    assert_eq!(file.args, off.args);
}

#[test]
fn a_flatpak_target_shares_the_roms_folder_and_loads_the_core_by_file_name() {
    let install = Install::Flatpak {
        script: PathBuf::from("/var/lib/flatpak/exports/bin/org.libretro.RetroArch"),
    };
    let core = snes9x(
        "/home/me/.var/app/org.libretro.RetroArch/config/retroarch/cores/snes9x_libretro.so",
    );
    let on = retroarch_target(&install, &core, Path::new(ROM), true, Path::new(QUIT)).unwrap();
    assert_eq!(on.exe, "flatpak");
    assert_eq!(
        on.args,
        strings(&[
            "run",
            "--filesystem=/games/snes",
            "--filesystem=/home/me/.local/share/wineo:ro",
            "org.libretro.RetroArch",
            "--appendconfig",
            QUIT,
            "-f",
            "-L",
            "snes9x_libretro.so",
            ROM
        ])
    );
    assert_eq!(
        crate::emulator::flatpak_app(&on.exe, &on.args),
        Some("org.libretro.RetroArch")
    );
    let off = retroarch_target(&install, &core, Path::new(ROM), false, Path::new(QUIT)).unwrap();
    assert!(!off.args.contains(&"-f".to_owned()));
    assert_eq!(off.args.len(), on.args.len() - 1);
}

#[test]
fn a_quit_file_path_with_a_pipe_is_left_out() {
    let core = snes9x("/usr/lib/libretro/snes9x_libretro.so");
    let quit = Path::new("/home/me/a|b/wineo/retroarch-quit.cfg");
    let native = Install::Path {
        name: "retroarch",
        found: PathBuf::from("/usr/bin/retroarch"),
    };
    let command = retroarch_target(&native, &core, Path::new(ROM), true, quit).unwrap();
    assert_eq!(
        command.args,
        strings(&["-f", "-L", "/usr/lib/libretro/snes9x_libretro.so", ROM])
    );
    let flatpak = Install::Flatpak {
        script: PathBuf::from("/var/lib/flatpak/exports/bin/org.libretro.RetroArch"),
    };
    let command = retroarch_target(&flatpak, &core, Path::new(ROM), false, quit).unwrap();
    assert!(!command.args.iter().any(|arg| arg == "--appendconfig"));
    assert!(!command.args.iter().any(|arg| arg.contains('|')));
}

#[test]
fn a_relative_rom_is_refused_for_retroarch_too() {
    let core = snes9x("/usr/lib/libretro/snes9x_libretro.so");
    let native = Install::Path {
        name: "retroarch",
        found: PathBuf::from("/usr/bin/retroarch"),
    };
    let error = retroarch_target(&native, &core, Path::new("-b"), true, Path::new(QUIT))
        .unwrap_err()
        .to_string();
    assert!(error.contains("isn't absolute"), "{error}");
}

const ON: &str = "quit_on_close_content = \"2\"\nconfig_save_on_exit = \"false\"\n";

#[test]
fn the_quit_file_holds_the_two_settings_when_on_and_nothing_when_off() {
    assert_eq!(quit_file_contents(true), ON);
    assert_eq!(quit_file_contents(false), "");
    let f = Fixture::new();
    assert_eq!(
        quit_file(&f.paths),
        f.home(".local/share/wineo/retroarch-quit.cfg")
    );
}

#[test]
fn the_quit_file_is_rewritten_only_when_it_differs() {
    let f = Fixture::new();
    let path = quit_file(&f.paths);
    assert!(write_quit_file(&f.paths, true).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), ON);
    let inode = fs::metadata(&path).unwrap().ino();
    assert!(!write_quit_file(&f.paths, true).unwrap());
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);

    assert!(write_quit_file(&f.paths, false).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), "");
    assert!(!write_quit_file(&f.paths, false).unwrap());

    fs::write(&path, "quit_on_close_content = \"1\"\n").unwrap();
    assert!(write_quit_file(&f.paths, true).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), ON);
    assert_eq!(refresh_quit_file(&f.paths, true), None);
}

#[test]
fn a_quit_file_that_cant_be_written_is_a_warning() {
    let f = Fixture::new();
    // wineo's data folder is a file, so nothing can go inside it.
    write(&f.home(".local/share/wineo"), "");
    let warning = refresh_quit_file(&f.paths, true).unwrap();
    assert!(
        warning.starts_with("Can't write RetroArch's quit file: "),
        "{warning}"
    );
    assert!(warning.ends_with('.'), "{warning}");
}

#[test]
fn a_targets_core_is_read_from_l_or_libretro() {
    let args = |words: &[&str]| strings(words);
    assert_eq!(
        target_core(&args(&[
            "--appendconfig",
            "x",
            "-L",
            "/c/a_libretro.so",
            "/r"
        ])),
        Some("/c/a_libretro.so")
    );
    assert_eq!(
        target_core(&args(&["--libretro", "/c/b_libretro.so", "/r"])),
        Some("/c/b_libretro.so")
    );
    assert_eq!(
        target_core(&args(&["--libretro=/c/c_libretro.so", "/r"])),
        Some("/c/c_libretro.so")
    );
    assert_eq!(target_core(&args(&["-f", "/r"])), None);
    assert_eq!(target_core(&args(&["-L"])), None);
}

#[test]
fn an_info_file_with_a_bom_and_comments_after_values_is_read() {
    let parsed = parse_info(concat!(
        "\u{feff}display_name = \"Snes9x\" # the name\n",
        "systemid = \"super_nes\"# no space\n",
        "supported_extensions = \"smc|sfc\" # \"quoted\" note\n",
    ));
    assert_eq!(parsed.display_name.as_deref(), Some("Snes9x"));
    assert_eq!(parsed.systemid.as_deref(), Some("super_nes"));
    assert_eq!(parsed.extensions, ["smc", "sfc"]);
}

#[test]
fn an_info_value_with_no_closing_quote_keeps_the_rest_of_the_line() {
    let parsed = parse_info("display_name = \"Snes9x\nsystemid = \"super_nes\"\n");
    assert_eq!(parsed.display_name.as_deref(), Some("Snes9x"));
    assert_eq!(parsed.systemid.as_deref(), Some("super_nes"));
}

#[test]
fn database_names_map_to_wineos_systems() {
    for (name, system) in [
        (
            "Nintendo - Super Nintendo Entertainment System",
            Some("snes"),
        ),
        ("Nintendo - Satellaview", Some("snes")),
        ("Nintendo - Family Computer Disk System", Some("nes")),
        ("Nintendo - Game Boy Color", Some("gbc")),
        ("Nintendo - Wii (Digital)", Some("wii")),
        ("Sega - Master System - Mark III", Some("mastersystem")),
        ("NEC - PC Engine SuperGrafx", Some("pcengine")),
        ("NEC - PC Engine CD - TurboGrafx-CD", Some("pcengine")),
        ("SNK - Neo Geo Pocket Color", Some("ngpc")),
        ("Bandai - WonderSwan Color", Some("wonderswancolor")),
        ("Sega - Mega-CD - Sega CD", None),
        ("Sega - 32X", None),
        ("Sega - Naomi", None),
        ("Atomiswave", None),
        ("MAME", None),
        ("nintendo - game boy", None),
    ] {
        assert_eq!(system_for_database(name), system, "{name}");
    }
}

#[test]
fn a_core_is_multi_system_only_when_its_databases_name_two_of_wineos() {
    let core = |systemid: &str, extensions: &str, database: &str| Core {
        path: PathBuf::from("/cores/x_libretro.so"),
        info: Some(parse_info(&info("X", systemid, extensions, database))),
    };
    let snes9x = core(
        "super_nes",
        "smc|sfc|swc|fig|bs|st",
        "Nintendo - Super Nintendo Entertainment System|Nintendo - Sufami Turbo|Nintendo - Satellaview",
    );
    let fceumm = core(
        "nes",
        "fds|nes|unif|unf",
        "Nintendo - Nintendo Entertainment System|Nintendo - Family Computer Disk System",
    );
    let flycast = core(
        "dreamcast",
        "chd|cdi|bin|cue|gdi|zip",
        "Sega - Dreamcast|Sega - Naomi|Sega - Naomi 2|Atomiswave",
    );
    let pce = core(
        "pc_engine",
        "pce|sgx|cue|ccd|chd",
        "NEC - PC Engine SuperGrafx|NEC - PC Engine - TurboGrafx 16|NEC - PC Engine CD - TurboGrafx-CD",
    );
    let ngp = core(
        "neo_geo_pocket",
        "ngp|ngc|ngpc|npc",
        "SNK - Neo Geo Pocket|SNK - Neo Geo Pocket Color",
    );
    let wswan = core(
        "wonderswan",
        "ws|wsc|pc2|pcv2",
        "Bandai - WonderSwan|Bandai - WonderSwan Color",
    );
    let picodrive = core(
        "mega_drive",
        "bin|gen|smd|md|32x|cue|sms|gg",
        "Sega - Master System - Mark III|Sega - Game Gear|Sega - Mega-CD - Sega CD|Sega - Mega Drive - Genesis|Sega - 32X",
    );
    for (core, extension, system) in [
        // One system to wineo, so every file is it.
        (&snes9x, "bs", Some("snes")),
        (&snes9x, "st", Some("snes")),
        (&snes9x, "swc", Some("snes")),
        (&fceumm, "unif", Some("nes")),
        (&fceumm, "fds", Some("nes")),
        (&flycast, "bin", Some("dreamcast")),
        (&flycast, "zip", Some("dreamcast")),
        (&pce, "cue", Some("pcengine")),
        (&pce, "sgx", Some("pcengine")),
        // Several, so by extension.
        (&ngp, "ngp", Some("ngp")),
        (&ngp, "ngc", Some("ngpc")),
        (&ngp, "ngpc", Some("ngpc")),
        (&ngp, "npc", None),
        (&wswan, "ws", Some("wonderswan")),
        (&wswan, "wsc", Some("wonderswancolor")),
        (&wswan, "pc2", None),
        (&picodrive, "md", Some("genesis")),
        (&picodrive, "sms", Some("mastersystem")),
        (&picodrive, "gg", Some("gamegear")),
        (&picodrive, "32x", None),
        (&picodrive, "cue", None),
    ] {
        assert_eq!(
            core.file_system(Some(extension)),
            system,
            "{:?} {extension}",
            core.info.as_ref().unwrap().database
        );
    }
}

#[test]
fn extensions_that_name_one_system_place_files() {
    for (extension, system) in [
        ("swc", "snes"),
        ("fig", "snes"),
        ("bs", "snes"),
        ("st", "snes"),
        ("fds", "nes"),
        ("ndd", "n64"),
        ("cdi", "dreamcast"),
        ("gdi", "dreamcast"),
        ("ngp", "ngp"),
        ("ngc", "ngpc"),
        ("ws", "wonderswan"),
        ("wsc", "wonderswancolor"),
        ("sgb", "gb"),
        ("srl", "nds"),
        ("wad", "wii"),
        ("sms", "mastersystem"),
    ] {
        assert_eq!(system_for_extension(extension), Some(system), "{extension}");
    }
    for generic in ["bin", "iso", "cue", "chd", "zip", "32x", "SFC"] {
        assert_eq!(system_for_extension(generic), None, "{generic}");
    }
}

#[test]
fn a_core_path_that_isnt_utf8_is_named_in_the_error() {
    use std::os::unix::ffi::OsStrExt;
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(b"/cores/sn\xffes_libretro.so"));
    let core = Core { path, info: None };
    let native = Install::Path {
        name: "retroarch",
        found: PathBuf::from("/usr/bin/retroarch"),
    };
    let flatpak = Install::Flatpak {
        script: PathBuf::from("/var/lib/flatpak/exports/bin/org.libretro.RetroArch"),
    };
    for install in [native, flatpak] {
        let error = retroarch_target(&install, &core, Path::new(ROM), true, Path::new(QUIT))
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "Core path `/cores/sn\u{fffd}es_libretro.so` isn't valid UTF-8."
        );
    }
}

#[test]
fn a_quit_file_path_that_isnt_utf8_is_left_out() {
    use std::os::unix::ffi::OsStrExt;
    let quit = Path::new(std::ffi::OsStr::from_bytes(
        b"/home/m\xffe/.local/share/wineo/retroarch-quit.cfg",
    ));
    let core = snes9x("/usr/lib/libretro/snes9x_libretro.so");
    let native = Install::Path {
        name: "retroarch",
        found: PathBuf::from("/usr/bin/retroarch"),
    };
    let command = retroarch_target(&native, &core, Path::new(ROM), true, quit).unwrap();
    assert_eq!(
        command.args,
        strings(&["-f", "-L", "/usr/lib/libretro/snes9x_libretro.so", ROM])
    );
    let flatpak = Install::Flatpak {
        script: PathBuf::from("/var/lib/flatpak/exports/bin/org.libretro.RetroArch"),
    };
    let command = retroarch_target(&flatpak, &core, Path::new(ROM), false, quit).unwrap();
    assert_eq!(
        command.args,
        strings(&[
            "run",
            "--filesystem=/games/snes",
            "org.libretro.RetroArch",
            "-L",
            "snes9x_libretro.so",
            ROM
        ])
    );
}

#[test]
fn the_flatpak_shares_the_quit_files_folder_read_only_and_escaped() {
    let core = snes9x("/c/snes9x_libretro.so");
    let flatpak = Install::Flatpak {
        script: PathBuf::from("/var/lib/flatpak/exports/bin/org.libretro.RetroArch"),
    };
    let quit = "/home/a:b\\c/wineo/retroarch-quit.cfg";
    let command =
        retroarch_target(&flatpak, &core, Path::new(ROM), false, Path::new(quit)).unwrap();
    assert_eq!(
        command.args,
        strings(&[
            "run",
            "--filesystem=/games/snes",
            "--filesystem=/home/a\\:b\\\\c/wineo:ro",
            "org.libretro.RetroArch",
            "--appendconfig",
            quit,
            "-L",
            "snes9x_libretro.so",
            ROM
        ])
    );
    // A native RetroArch reads it without a share.
    let native = Install::Path {
        name: "retroarch",
        found: PathBuf::from("/usr/bin/retroarch"),
    };
    let command = retroarch_target(&native, &core, Path::new(ROM), false, Path::new(quit)).unwrap();
    assert!(
        !command
            .args
            .iter()
            .any(|arg| arg.starts_with("--filesystem"))
    );
    assert_eq!(command.args[..2], strings(&["--appendconfig", quit]));
}

#[test]
fn a_quit_file_folder_flatpak_would_refuse_leaves_appendconfig_out() {
    let core = snes9x("/c/snes9x_libretro.so");
    let flatpak = Install::Flatpak {
        script: PathBuf::from("/var/lib/flatpak/exports/bin/org.libretro.RetroArch"),
    };
    for quit in [
        "/retroarch-quit.cfg",
        "/home/me/../me/wineo/retroarch-quit.cfg",
        "wineo/retroarch-quit.cfg",
    ] {
        let command =
            retroarch_target(&flatpak, &core, Path::new(ROM), false, Path::new(quit)).unwrap();
        assert_eq!(
            command.args,
            strings(&[
                "run",
                "--filesystem=/games/snes",
                "org.libretro.RetroArch",
                "-L",
                "snes9x_libretro.so",
                ROM
            ]),
            "{quit}"
        );
    }
    // A native RetroArch still gets them, since nothing is shared.
    let native = Install::Path {
        name: "retroarch",
        found: PathBuf::from("/usr/bin/retroarch"),
    };
    let command = retroarch_target(
        &native,
        &core,
        Path::new(ROM),
        false,
        Path::new("/retroarch-quit.cfg"),
    )
    .unwrap();
    assert_eq!(
        command.args[..2],
        strings(&["--appendconfig", "/retroarch-quit.cfg"])
    );
}
