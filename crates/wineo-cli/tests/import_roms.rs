//! `wineo import roms` against fake emulators, a fake RetroArch with fixture
//! cores, and folders of empty ROM files. Every search root is inside the
//! fixture: PATH holds only the fakes, and system folders hang off
//! `WINEO_TEST_SYSTEM_ROOT`.

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::pty::Pty;
use common::{Fixture, assert_success, stderr, stdout, write_script};
use serde_json::Value;

struct Roms {
    f: Fixture,
    bin: PathBuf,
    sys: PathBuf,
}

const FAKE_EMULATOR: &str = "#!/bin/sh\nexit 0\n";

impl Roms {
    fn new() -> Self {
        let f = Fixture::new();
        let bin = f.root.path().join("emulators");
        let sys = f.root.path().join("sys");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&sys).unwrap();
        Self { f, bin, sys }
    }

    fn root(&self) -> &Path {
        self.f.root.path()
    }

    fn emulator(&self, name: &str) -> PathBuf {
        let path = self.bin.join(name);
        write_script(&path, FAKE_EMULATOR);
        path
    }

    /// A RetroArch core with its info file in the distro folders.
    fn core(&self, id: &str, display: &str, extensions: &str) {
        self.core_in(&self.sys.join("usr/lib/libretro"), id, display, extensions);
    }

    /// A core in `cores`, with its info file in the distro folders.
    fn core_in(&self, cores: &Path, id: &str, display: &str, extensions: &str) {
        let info = self.sys.join("usr/share/libretro/info");
        fs::create_dir_all(cores).unwrap();
        fs::create_dir_all(&info).unwrap();
        fs::write(cores.join(format!("{id}_libretro.so")), "").unwrap();
        fs::write(
            info.join(format!("{id}_libretro.info")),
            format!(
                "display_name = \"{display}\"\nsupported_extensions = \"{extensions}\"\nsystemid = \"super_nes\"\n"
            ),
        )
        .unwrap();
    }

    /// Empty ROM files in `folder` under the fixture.
    fn folder(&self, folder: &str, files: &[&str]) -> String {
        let dir = self.root().join(folder);
        fs::create_dir_all(&dir).unwrap();
        for file in files {
            fs::write(dir.join(file), "").unwrap();
        }
        dir.display().to_string()
    }

    fn command(&self) -> Command {
        let mut command = self.f.wineo();
        let path =
            std::env::join_paths([self.bin.as_os_str(), self.f.sandbox.as_os_str()]).unwrap();
        command
            .env("HOME", self.root())
            .env("PATH", path)
            .env("WINEO_TEST_SYSTEM_ROOT", &self.sys);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command()
            .arg("import")
            .arg("roms")
            .args(args)
            .output()
            .unwrap()
    }

    fn pty(&self, args: &[&str]) -> Pty {
        let mut command = self.command();
        command.args(["import", "roms"]).args(args);
        Pty::spawn(command)
    }

    fn config(&self, name: &str) -> String {
        self.f.created_config(name)
    }

    fn environments(&self) -> BTreeSet<String> {
        let dir = self.root().join("config/wineo/environments");
        fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

fn failed_with(output: &Output, code: i32, text: &str) {
    assert_eq!(output.status.code(), Some(code), "{}", stderr(output));
    assert!(stderr(output).contains(text), "{}", stderr(output));
}

fn gamecube(r: &Roms) -> String {
    r.emulator("dolphin-emu");
    r.folder(
        "roms/gc",
        &["Metroid Prime (USA).rvz", "Zelda [!].gcm", "notes.txt"],
    )
}

#[test]
fn emulators_lists_what_was_found_and_retroarchs_cores() {
    let r = Roms::new();
    r.emulator("dolphin-emu");
    r.emulator("retroarch");
    r.core("snes9x", "Nintendo - SNES / SFC (Snes9x)", "smc|sfc");
    let output = r.run(&["--emulators"]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("dolphin    Dolphin"), "{out}");
    assert!(out.contains("retroarch  RetroArch"), "{out}");
    assert!(!out.contains("pcsx2"), "{out}");
    assert!(out.contains("Cores for RetroArch (retroarch):"), "{out}");
    assert!(
        out.contains("snes9x  Nintendo - SNES / SFC (Snes9x)  snes"),
        "{out}"
    );
}

#[test]
fn emulators_finds_none_on_an_empty_path() {
    let r = Roms::new();
    let output = r.run(&["--emulators"]);
    assert_success(&output);
    assert_eq!(stdout(&output), "wineo found no emulators.\n");
}

#[test]
fn emulators_json_has_its_documented_keys() {
    let r = Roms::new();
    r.emulator("dolphin-emu");
    r.emulator("retroarch");
    r.core("snes9x", "Nintendo - SNES / SFC (Snes9x)", "smc|sfc");
    // A user Flatpak install of PCSX2, by its export script.
    let exports = r.root().join("data/flatpak/exports/bin");
    fs::create_dir_all(&exports).unwrap();
    write_script(exports.join("net.pcsx2.PCSX2"), FAKE_EMULATOR);
    let output = r.run(&["--emulators", "--json"]);
    assert_success(&output);
    let json: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(keys(&json), ["emulators"]);
    let emulators = json["emulators"].as_array().unwrap();
    let ids: Vec<&str> = emulators
        .iter()
        .map(|emulator| emulator["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["dolphin", "retroarch", "pcsx2"]);
    assert_eq!(keys(&emulators[0]), ["id", "installs", "name"]);
    let dolphin = &emulators[0]["installs"][0];
    assert_eq!(keys(dolphin), ["app", "exe", "kind", "path"]);
    assert_eq!(dolphin["kind"], "path");
    assert_eq!(dolphin["exe"], "dolphin-emu");
    let retroarch = &emulators[1]["installs"][0];
    assert_eq!(keys(retroarch), ["app", "cores", "exe", "kind", "path"]);
    let core = &retroarch["cores"][0];
    assert_eq!(keys(core), ["extensions", "file", "id", "name", "system"]);
    assert_eq!(core["id"], "snes9x");
    assert_eq!(core["system"], "snes");
    assert_eq!(core["extensions"], serde_json::json!(["smc", "sfc"]));
    let pcsx2 = &emulators[2]["installs"][0];
    assert_eq!(pcsx2["kind"], "flatpak");
    assert_eq!(pcsx2["app"], "net.pcsx2.PCSX2");
    assert_eq!(pcsx2["exe"], Value::Null);
}

#[test]
fn list_shows_games_and_what_is_imported() {
    let r = Roms::new();
    let folder = gamecube(&r);
    assert_success(&r.run(&[&folder, "--emulator", "dolphin", "--game", "Zelda"]));
    let output = r.run(&[&folder, "--emulator", "dolphin", "--list"]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("TITLE"), "{out}");
    assert!(
        out.contains("Metroid Prime  gamecube or wii  Metroid Prime (USA).rvz"),
        "{out}"
    );
    assert!(
        out.contains("Zelda          gamecube         Zelda [!].gcm"),
        "{out}"
    );
    assert!(out.lines().last().unwrap().ends_with("zelda"), "{out}");
    assert!(!out.contains("notes"), "{out}");
}

#[test]
fn list_json_has_its_documented_keys() {
    let r = Roms::new();
    let folder = gamecube(&r);
    assert_success(&r.run(&[&folder, "--emulator", "dolphin", "--game", "Zelda"]));
    let output = r.run(&[&folder, "--emulator", "dolphin", "--list", "--json"]);
    assert_success(&output);
    let json: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(keys(&json), ["core", "emulator", "games", "skipped"]);
    assert_eq!(json["emulator"], "dolphin");
    assert_eq!(json["core"], Value::Null);
    assert_eq!(json["skipped"], serde_json::json!([]));
    let games = json["games"].as_array().unwrap();
    assert_eq!(games.len(), 2);
    for game in games {
        assert_eq!(
            keys(game),
            [
                "ambiguous",
                "environment",
                "file",
                "reason",
                "rom",
                "state",
                "system",
                "title"
            ]
        );
    }
    assert_eq!(games[0]["title"], "Metroid Prime");
    assert_eq!(games[0]["ambiguous"], true);
    assert_eq!(games[0]["system"], Value::Null);
    assert_eq!(games[0]["state"], "new");
    assert_eq!(games[1]["state"], "imported");
    assert_eq!(games[1]["environment"], "zelda");
    assert_eq!(games[1]["system"], "gamecube");
    assert_eq!(games[1]["file"], "Zelda [!].gcm");

    // --system answers the files that could be either.
    let output = r.run(&[
        &folder,
        "--emulator",
        "dolphin",
        "--list",
        "--json",
        "--system",
        "wii",
    ]);
    let json: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(json["games"][0]["system"], "wii");
}

#[test]
fn json_is_refused_without_list_or_emulators() {
    let r = Roms::new();
    let folder = gamecube(&r);
    for extra in [
        &["--all", "--yes"][..],
        &["--game", "Zelda"],
        &["--dry-run"],
    ] {
        let mut args = vec![folder.as_str(), "--emulator", "dolphin", "--json"];
        args.extend(extra);
        let output = r.run(&args);
        failed_with(&output, 2, "<--list|--emulators>");
    }
    assert!(r.environments().is_empty());
}

#[test]
fn an_unknown_emulator_is_refused_with_the_known_ones() {
    let r = Roms::new();
    let folder = gamecube(&r);
    let output = r.run(&[&folder, "--emulator", "Dolphin", "--all", "--yes"]);
    failed_with(
        &output,
        1,
        "Unknown emulator `Dolphin`. Known: dolphin, retroarch, cemu, pcsx2, duckstation, ppsspp, rpcs3, melonds, mgba, xemu, azahar.",
    );
}

#[test]
fn a_missing_emulator_points_at_exe() {
    let r = Roms::new();
    let folder = r.folder("roms/gba", &["Golden Sun.gba"]);
    let output = r.run(&[&folder, "--emulator", "mgba", "--all", "--yes"]);
    failed_with(
        &output,
        1,
        "mGBA wasn't found. Install it, or pass --exe PATH.",
    );
}

#[test]
fn custom_and_emulator_are_refused_together() {
    let r = Roms::new();
    let folder = r.folder("roms/switch", &["Game.nsp"]);
    let output = r.run(&[
        &folder,
        "--custom",
        "eden",
        "--args",
        "{rom}",
        "--ext",
        "nsp",
        "--emulator",
        "dolphin",
    ]);
    failed_with(&output, 2, "cannot be used with");
}

#[test]
fn custom_needs_args_and_ext() {
    let r = Roms::new();
    let folder = r.folder("roms/switch", &["Game.nsp"]);
    let output = r.run(&[
        &folder, "--custom", "eden", "--ext", "nsp", "--all", "--yes",
    ]);
    failed_with(&output, 2, "--args <ARGS>");
    let output = r.run(&[
        &folder, "--custom", "eden", "--args", "{rom}", "--all", "--yes",
    ]);
    failed_with(&output, 2, "--ext <EXT>");
    // --args and --ext mean nothing without --custom.
    let output = r.run(&[&folder, "--emulator", "dolphin", "--args", "{rom}"]);
    failed_with(&output, 2, "--custom <EXE>");
}

#[test]
fn custom_imports_with_its_arguments_and_system() {
    let r = Roms::new();
    let eden = r.emulator("eden.AppImage");
    let folder = r.folder(
        "roms/switch",
        &["Mario Odyssey [0100].nsp", "Other.xci", "x.txt"],
    );
    let output = r.run(&[
        &folder,
        "--custom",
        eden.to_str().unwrap(),
        "--args",
        "-f -g {rom}",
        "--ext",
        "nsp,.XCI",
        "--system",
        "Switch",
        "--all",
        "--yes",
    ]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("Imported 2 of 2 games."),
        "{}",
        stdout(&output)
    );
    let config = r.config("mario-odyssey");
    assert!(config.contains("emulator = \"custom\""), "{config}");
    assert!(config.contains("tags = [\"switch\"]"), "{config}");
    let rom = Path::new(&folder).join("Mario Odyssey [0100].nsp");
    assert!(
        config.contains(&format!("args = [\"-f\", \"-g\", \"{}\"]", rom.display())),
        "{config}"
    );
    assert!(r.config("other").contains("Other.xci"));
}

#[test]
fn custom_arguments_need_the_rom_placeholder() {
    let r = Roms::new();
    let eden = r.emulator("eden");
    let folder = r.folder("roms/switch", &["Game.nsp"]);
    let output = r.run(&[
        &folder,
        "--custom",
        eden.to_str().unwrap(),
        "--args",
        "-f",
        "--ext",
        "nsp",
        "--all",
        "--yes",
    ]);
    failed_with(&output, 1, "The arguments need {rom} for the game.");
    assert!(r.environments().is_empty());
}

#[test]
fn exe_runs_a_built_in_from_another_file() {
    let r = Roms::new();
    let cemu = r.emulator("Cemu.AppImage");
    let folder = r.folder("roms/wiiu", &["Zelda BOTW.wua"]);
    let output = r.run(&[
        &folder,
        "--emulator",
        "cemu",
        "--exe",
        "~/emulators/Cemu.AppImage",
        "--all",
        "--yes",
    ]);
    assert_success(&output);
    let config = r.config("zelda-botw");
    assert!(
        config.contains("exe = \"~/emulators/Cemu.AppImage\""),
        "{config}"
    );
    assert!(config.contains("emulator = \"cemu\""), "{config}");
    assert!(config.contains("tags = [\"wiiu\"]"), "{config}");
    assert!(cemu.is_file());
    // --exe belongs to --emulator.
    let output = r.run(&[&folder, "--exe", "/x", "--all", "--yes"]);
    failed_with(&output, 2, "--emulator <ID>");
}

#[test]
fn core_is_required_when_several_match() {
    let r = Roms::new();
    r.emulator("retroarch");
    r.core("snes9x", "Nintendo - SNES / SFC (Snes9x)", "smc|sfc");
    r.core("bsnes", "Nintendo - SNES / SFC (bsnes)", "sfc");
    r.core("mgba", "Nintendo - Game Boy Advance (mGBA)", "gba");
    let folder = r.folder("roms/snes", &["Super Metroid (USA).sfc"]);
    let output = r.run(&[&folder, "--emulator", "retroarch", "--all", "--yes"]);
    failed_with(&output, 1, "pass --core with one of: bsnes, snes9x");
    assert!(r.environments().is_empty());
}

#[test]
fn core_picks_by_id_display_name_or_file_name() {
    for (query, file) in [
        ("snes9x", "snes9x_libretro.so"),
        ("nintendo - snes / sfc (bsnes)", "bsnes_libretro.so"),
        ("snes9x_libretro.so", "snes9x_libretro.so"),
    ] {
        let r = Roms::new();
        r.emulator("retroarch");
        r.core("snes9x", "Nintendo - SNES / SFC (Snes9x)", "smc|sfc");
        r.core("bsnes", "Nintendo - SNES / SFC (bsnes)", "sfc");
        let folder = r.folder("roms/snes", &["Super Metroid (USA).sfc"]);
        let output = r.run(&[
            &folder,
            "--emulator",
            "retroarch",
            "--core",
            query,
            "--all",
            "--yes",
        ]);
        assert_success(&output);
        let config = r.config("super-metroid");
        assert!(config.contains(file), "{query}: {config}");
        assert!(config.contains("--appendconfig"), "{config}");
        assert!(config.contains("tags = [\"snes\"]"), "{config}");
    }
}

#[test]
fn one_matching_core_needs_no_flag_and_unknown_cores_are_refused() {
    let r = Roms::new();
    r.emulator("retroarch");
    r.core("snes9x", "Nintendo - SNES / SFC (Snes9x)", "smc|sfc");
    r.core("mgba", "Nintendo - Game Boy Advance (mGBA)", "gba");
    let folder = r.folder("roms/snes", &["Super Metroid (USA).sfc"]);
    let output = r.run(&[
        &folder,
        "--emulator",
        "retroarch",
        "--core",
        "nope",
        "--list",
    ]);
    failed_with(
        &output,
        1,
        "No RetroArch core named `nope`. Installed: mgba, snes9x.",
    );
    let output = r.run(&[&folder, "--emulator", "retroarch", "--list", "--json"]);
    assert_success(&output);
    let json: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(json["core"], "snes9x");
    // --core means nothing to another emulator.
    r.emulator("dolphin-emu");
    let output = r.run(&[
        &folder,
        "--emulator",
        "dolphin",
        "--core",
        "snes9x",
        "--list",
    ]);
    failed_with(&output, 1, "--core is only for RetroArch.");
}

#[test]
fn game_matches_the_title_or_the_file_name() {
    let r = Roms::new();
    let folder = gamecube(&r);
    let output = r.run(&[&folder, "--emulator", "dolphin", "--game", "zelda"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("Imported zelda."),
        "{}",
        stdout(&output)
    );
    let output = r.run(&[
        &folder,
        "--emulator",
        "dolphin",
        "--system",
        "wii",
        "--game",
        "Metroid Prime (USA).rvz",
    ]);
    assert_success(&output);
    assert!(r.config("metroid-prime").contains("tags = [\"wii\"]"));
    let output = r.run(&[&folder, "--emulator", "dolphin", "--game", "Mario"]);
    failed_with(&output, 1, "matches `Mario`.");
}

#[test]
fn a_game_whose_file_could_be_either_needs_a_system() {
    let r = Roms::new();
    let folder = gamecube(&r);
    let output = r.run(&[&folder, "--emulator", "dolphin", "--game", "Metroid Prime"]);
    failed_with(
        &output,
        1,
        "pass --system gamecube or --system wii for the games whose files could be either",
    );
    assert!(r.environments().is_empty());
}

#[test]
fn all_needs_yes_without_a_terminal_and_a_rerun_skips_imported_games() {
    let r = Roms::new();
    let folder = gamecube(&r);
    let output = r.run(&[
        &folder,
        "--emulator",
        "dolphin",
        "--system",
        "gamecube",
        "--all",
    ]);
    failed_with(
        &output,
        1,
        "bulk ROM import needs a terminal; pass --yes to skip the confirmation",
    );
    assert!(r.environments().is_empty());

    let output = r.run(&[
        &folder,
        "--emulator",
        "dolphin",
        "--system",
        "gamecube",
        "--all",
        "--yes",
    ]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("Imported 2 of 2 games."),
        "{}",
        stdout(&output)
    );
    assert_eq!(
        r.environments(),
        BTreeSet::from(["metroid-prime.toml".to_owned(), "zelda.toml".to_owned()])
    );

    fs::write(Path::new(&folder).join("Pikmin.gcm"), "").unwrap();
    let output = r.run(&[
        &folder,
        "--emulator",
        "dolphin",
        "--system",
        "gamecube",
        "--all",
        "--yes",
    ]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("2 already imported: metroid-prime, zelda"),
        "{out}"
    );
    assert!(out.contains("Imported 1 of 1 game."), "{out}");
    assert_eq!(r.environments().len(), 3);
}

#[test]
fn dry_run_writes_nothing() {
    let r = Roms::new();
    let folder = gamecube(&r);
    let output = r.run(&[
        &folder,
        "--emulator",
        "dolphin",
        "--system",
        "gamecube",
        "--all",
        "--dry-run",
    ]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("metroid-prime  Metroid Prime  gamecube"),
        "{out}"
    );
    assert!(out.ends_with("Dry run, nothing was imported.\n"), "{out}");
    let rom = Path::new(&folder).join("Zelda [!].gcm");
    let output = r.run(&[rom.to_str().unwrap(), "--emulator", "dolphin", "--dry-run"]);
    assert_success(&output);
    assert!(stdout(&output).contains("Ready to import zelda:"));
    assert!(!r.root().join("config/wineo/environments").exists());
    assert!(!r.root().join("config/wineo/library.toml").exists());
}

#[test]
fn a_single_file_imports_that_game() {
    let r = Roms::new();
    r.emulator("mgba");
    let folder = r.folder("roms/gb", &["Tetris (World).gb", "Pokemon Red.gb"]);
    let rom = Path::new(&folder).join("Tetris (World).gb");
    let output = r.run(&[rom.to_str().unwrap(), "--emulator", "mgba"]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("Ready to import tetris:"), "{out}");
    assert!(out.contains("Imported tetris."), "{out}");
    assert_eq!(r.environments(), BTreeSet::from(["tetris.toml".to_owned()]));
    let config = r.config("tetris");
    assert!(
        config.contains(&format!("args = [\"-f\", \"{}\"]", rom.display())),
        "{config}"
    );
    let output = r.run(&[rom.to_str().unwrap(), "--emulator", "mgba"]);
    assert_success(&output);
    assert_eq!(stdout(&output), "Already imported as tetris.\n");
}

#[test]
fn no_fullscreen_leaves_the_flag_out() {
    let r = Roms::new();
    r.emulator("mgba");
    let folder = r.folder("roms/gb", &["Tetris (World).gb"]);
    let output = r.run(&[
        &folder,
        "--emulator",
        "mgba",
        "--no-fullscreen",
        "--all",
        "--yes",
    ]);
    assert_success(&output);
    let rom = Path::new(&folder).join("Tetris (World).gb");
    let config = r.config("tetris");
    assert!(
        config.contains(&format!("args = [\"{}\"]", rom.display())),
        "{config}"
    );
}

#[test]
fn art_without_a_key_says_so_and_still_imports() {
    let r = Roms::new();
    r.emulator("mgba");
    let folder = r.folder("roms/gb", &["Tetris (World).gb"]);
    let output = r.run(&[&folder, "--emulator", "mgba", "--art", "--all", "--yes"]);
    assert_success(&output);
    assert!(
        stderr(&output).contains("note: run `wineo art key` to fetch art from SteamGridDB"),
        "{}",
        stderr(&output)
    );
    assert!(stdout(&output).contains("Imported tetris."));
}

#[test]
fn a_portable_root_is_refused() {
    let r = Roms::new();
    let folder = gamecube(&r);
    let portable = r.root().join("portable");
    for args in [
        vec![folder.as_str(), "--emulator", "dolphin", "--all", "--yes"],
        vec!["--emulators"],
        vec![folder.as_str(), "--emulator", "dolphin", "--list"],
    ] {
        let output = r
            .command()
            .arg("--portable")
            .arg(&portable)
            .args(["import", "roms"])
            .args(&args)
            .output()
            .unwrap();
        failed_with(
            &output,
            1,
            "Emulated games can't be imported into a portable folder.",
        );
    }
    assert!(!portable.join("environments").exists());
}

#[test]
fn bare_import_roms_asks_emulator_folder_system_and_game() {
    let r = Roms::new();
    let folder = gamecube(&r);
    let mut command = r.command();
    command.args(["import", "roms"]);
    let mut pty = Pty::spawn(command);
    pty.expect("Which emulator?");
    pty.expect("Dolphin (dolphin-emu)");
    pty.expect("Emulator: ");
    pty.send("1");
    pty.expect("Folder or file of games");
    pty.send(&folder);
    pty.expect("Which system are the games whose files could be either?");
    pty.expect("System: ");
    pty.send("2");
    pty.expect("Which game?");
    pty.expect("a) All 2 games");
    pty.expect("Game: ");
    pty.send("1");
    pty.expect("Ready to import metroid-prime:");
    pty.expect("Import it?");
    pty.send("");
    pty.expect("Imported metroid-prime.");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(r.config("metroid-prime").contains("tags = [\"wii\"]"));
}

#[test]
fn the_import_menu_offers_emulated_games_and_esc_returns_to_it() {
    let r = Roms::new();
    r.emulator("dolphin-emu");
    let mut command = r.command();
    // GOG's flow must never reach the real GOG.
    command
        .env("WINEO_TEST_GOG_API", "http://127.0.0.1:9")
        .arg("import");
    let mut pty = Pty::spawn(command);
    pty.expect("Import from where?");
    pty.expect("5) Emulated games");
    pty.expect("Source [1]: ");
    pty.send("5");
    pty.expect("Which emulator?");
    pty.expect("Esc to go back");
    pty.esc();
    pty.expect("Import from where?");
    pty.expect("Source [1]: ");
    pty.send("4");
    // The fixture has no Heroic, so GOG's flow stops at its first check.
    pty.expect("Heroic has no GOG games to migrate.");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(1), "{output}");
}

fn complete(r: &Roms, words: &[&str]) -> Vec<String> {
    let output = r
        .command()
        .env("COMPLETE", "fish")
        .env("_CLAP_COMPLETE_INDEX", (words.len() - 1).to_string())
        .env("_CLAP_IFS", "\n")
        .arg("--")
        .args(words)
        .output()
        .unwrap();
    assert_success(&output);
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| line.split('\t').next().unwrap().to_owned())
        .collect()
}

#[test]
fn completion_offers_emulators_systems_and_cores() {
    let r = Roms::new();
    r.emulator("retroarch");
    r.core("snes9x", "Nintendo - SNES / SFC (Snes9x)", "smc|sfc");
    r.core("bsnes", "Nintendo - SNES / SFC (bsnes)", "sfc");
    assert_eq!(
        complete(&r, &["wineo", "import", "roms", "--emulator", "d"]),
        ["dolphin", "duckstation"]
    );
    assert_eq!(
        complete(
            &r,
            &[
                "wineo",
                "import",
                "roms",
                "--emulator",
                "dolphin",
                "--system",
                ""
            ]
        ),
        ["gamecube", "wii"]
    );
    assert_eq!(
        complete(
            &r,
            &["wineo", "import", "roms", "--emulator=mgba", "--system", ""]
        ),
        ["gb", "gba", "gbc"]
    );
    let all = complete(&r, &["wineo", "import", "roms", "--system", ""]);
    assert!(
        all.contains(&"ps2".to_owned()) && all.contains(&"3ds".to_owned()),
        "{all:?}"
    );
    assert_eq!(
        complete(
            &r,
            &[
                "wineo",
                "import",
                "roms",
                "--emulator",
                "retroarch",
                "--core",
                ""
            ]
        ),
        ["bsnes", "snes9x"]
    );
    assert_eq!(
        complete(&r, &["wineo", "import", "roms", "--core", "s"]),
        ["snes9x"]
    );
    assert!(
        complete(
            &r,
            &[
                "wineo",
                "import",
                "roms",
                "--emulator",
                "dolphin",
                "--core",
                ""
            ]
        )
        .is_empty()
    );
    // An --exe RetroArch that isn't one leaves nothing to offer.
    assert!(
        complete(
            &r,
            &[
                "wineo",
                "import",
                "roms",
                "--emulator",
                "retroarch",
                "--exe",
                "/nope",
                "--core",
                ""
            ]
        )
        .is_empty()
    );
}

/// Two snes9x cores: the distro's, and one RetroArch downloaded into the
/// user's cores folder. The user's sorts first, by its path.
fn twin_cores(r: &Roms) -> (PathBuf, PathBuf) {
    r.emulator("retroarch");
    r.core("snes9x", "Nintendo - SNES / SFC (Snes9x)", "smc|sfc");
    let user = r.root().join("config/retroarch/cores");
    r.core_in(&user, "snes9x", "Nintendo - SNES / SFC (Snes9x)", "smc|sfc");
    (
        user.join("snes9x_libretro.so"),
        r.sys.join("usr/lib/libretro/snes9x_libretro.so"),
    )
}

#[test]
fn going_back_to_the_emulator_forgets_the_system_chosen_for_it() {
    let r = Roms::new();
    r.emulator("dolphin-emu");
    r.emulator("mgba");
    let folder = r.folder("roms/mixed", &["Metroid Prime (USA).rvz", "Tetris.gb"]);
    let mut pty = r.pty(&[]);
    pty.expect("Emulator: ");
    pty.send("1");
    pty.expect("Folder or file of games");
    pty.send(&folder);
    pty.expect("System: ");
    pty.send("2");
    pty.expect("Game: ");
    pty.esc();
    pty.expect("System");
    pty.esc();
    pty.expect("Folder or file of games");
    pty.esc();
    pty.expect("Emulator: ");
    pty.send("2");
    pty.expect("Folder or file of games");
    pty.send("");
    pty.expect("Which game?");
    pty.expect("Game: ");
    pty.send("1");
    pty.expect("Import it?");
    pty.send("");
    pty.expect("Imported tetris.");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(r.config("tetris").contains("tags = [\"gb\"]"));
}

#[test]
fn a_quoted_tilde_path_is_one_game() {
    let r = Roms::new();
    r.emulator("mgba");
    r.folder("roms/gb", &["Tetris.gb", "Other.gb"]);
    let output = r.run(&["~/roms/gb/Tetris.gb", "--emulator", "mgba"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("Imported tetris."),
        "{}",
        stdout(&output)
    );

    r.emulator("ppsspp");
    r.folder("roms/psp/Loco Roco", &["EBOOT.PBP"]);
    let output = r.run(&["~/roms/psp/Loco Roco", "--emulator", "ppsspp"]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("Imported loco-roco."),
        "{}",
        stdout(&output)
    );
}

fn non_utf8_file(dir: &str, name: &[u8]) {
    use std::os::unix::ffi::OsStrExt;
    fs::write(Path::new(dir).join(std::ffi::OsStr::from_bytes(name)), "").unwrap();
}

#[test]
fn list_json_reads_a_file_name_that_isnt_utf8() {
    let r = Roms::new();
    let folder = gamecube(&r);
    non_utf8_file(&folder, b"Bad\xff.gcm");
    let output = r.run(&[&folder, "--emulator", "dolphin", "--list", "--json"]);
    assert_success(&output);
    let json: Value = serde_json::from_str(&stdout(&output)).unwrap();
    let bad = &json["games"][0];
    assert_eq!(bad["state"], "unusable", "{json}");
    assert!(
        bad["rom"].as_str().unwrap().ends_with("Bad\u{fffd}.gcm"),
        "{json}"
    );
}

#[test]
fn twin_cores_need_a_path_without_a_terminal() {
    let r = Roms::new();
    let (user, distro) = twin_cores(&r);
    let folder = r.folder("roms/snes", &["Super Metroid.sfc"]);
    let output = r.run(&[
        &folder,
        "--emulator",
        "retroarch",
        "--core",
        "snes9x",
        "--all",
        "--yes",
    ]);
    failed_with(
        &output,
        1,
        &format!(
            "`snes9x` matches more than one RetroArch core: {}, {}. Pass --core with its path.",
            user.display(),
            distro.display()
        ),
    );
    let output = r.run(&[&folder, "--emulator", "retroarch", "--all", "--yes"]);
    failed_with(
        &output,
        1,
        &format!(
            "pass --core with one of: {}, {}",
            user.display(),
            distro.display()
        ),
    );
    let output = r.run(&[
        &folder,
        "--emulator",
        "retroarch",
        "--core",
        distro.to_str().unwrap(),
        "--all",
        "--yes",
    ]);
    assert_success(&output);
    let config = r.config("super-metroid");
    assert!(config.contains(&distro.display().to_string()), "{config}");
    let output = r.run(&[
        &folder,
        "--emulator",
        "retroarch",
        "--core",
        "~/config/retroarch/cores/snes9x_libretro.so",
        "--list",
        "--json",
    ]);
    assert_success(&output);
}

#[test]
fn twin_cores_are_asked_for_by_folder_in_a_terminal() {
    let r = Roms::new();
    let (user, distro) = twin_cores(&r);
    let folder = r.folder("roms/snes", &["Super Metroid.sfc"]);
    let rom = Path::new(&folder).join("Super Metroid.sfc");
    let mut pty = r.pty(&[
        rom.to_str().unwrap(),
        "--emulator",
        "retroarch",
        "--core",
        "snes9x",
    ]);
    pty.expect("Which core?");
    pty.expect(&format!("in {}", user.parent().unwrap().display()));
    pty.expect(&format!("in {}", distro.parent().unwrap().display()));
    pty.expect("Core: ");
    pty.send("2");
    pty.expect("Import it?");
    pty.send("");
    pty.expect("Imported super-metroid.");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(
        r.config("super-metroid")
            .contains(&distro.display().to_string())
    );
}

#[test]
fn emulators_shows_the_folder_of_twin_cores() {
    let r = Roms::new();
    let (user, distro) = twin_cores(&r);
    let output = r.run(&["--emulators"]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("FOLDER"), "{out}");
    assert!(
        out.contains(&user.parent().unwrap().display().to_string()),
        "{out}"
    );
    assert!(
        out.contains(&distro.parent().unwrap().display().to_string()),
        "{out}"
    );
}

#[test]
fn retroarch_asks_which_core_in_a_terminal() {
    let r = Roms::new();
    r.emulator("retroarch");
    r.core("snes9x", "Nintendo - SNES / SFC (Snes9x)", "smc|sfc");
    r.core("bsnes", "Nintendo - SNES / SFC (bsnes)", "sfc");
    let folder = r.folder("roms/snes", &["Super Metroid.sfc"]);
    let mut pty = r.pty(&[&folder, "--emulator", "retroarch", "--all"]);
    pty.expect("Which core?");
    pty.expect("1) Nintendo - SNES / SFC (bsnes) (bsnes)");
    pty.expect("Core: ");
    pty.send("1");
    pty.expect("Import 1 game?");
    pty.send("");
    pty.expect("Imported super-metroid.");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(r.config("super-metroid").contains("bsnes_libretro.so"));
}

#[test]
fn yes_skips_the_confirmation_after_questions() {
    let r = Roms::new();
    r.emulator("mgba");
    let folder = r.folder("roms/gb", &["Tetris.gb"]);
    let mut pty = r.pty(&["--yes"]);
    pty.expect("Emulator: ");
    pty.send("1");
    pty.expect("Folder or file of games");
    pty.send(&folder);
    pty.expect("Game: ");
    pty.send("1");
    pty.expect("Imported tetris.");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(!output.contains("Import it?"), "{output}");
}

#[test]
fn list_checks_system_against_the_emulator() {
    let r = Roms::new();
    r.emulator("mgba");
    let folder = r.folder("roms/gb", &["Tetris.gb"]);
    let output = r.run(&[&folder, "--emulator", "mgba", "--list", "--system", "wii"]);
    failed_with(&output, 1, "`wii` isn't a system mGBA plays: gb, gbc, gba.");
    r.emulator("retroarch");
    r.core("snes9x", "Nintendo - SNES / SFC (Snes9x)", "smc|sfc");
    let snes = r.folder("roms/snes", &["Super Metroid.sfc"]);
    let output = r.run(&[
        &snes,
        "--emulator",
        "retroarch",
        "--list",
        "--system",
        "snes",
    ]);
    failed_with(&output, 1, "RetroArch has no system to choose.");
}

#[test]
fn game_takes_a_path_and_names_paths_when_several_match() {
    let r = Roms::new();
    r.emulator("dolphin-emu");
    let folder = r.folder("roms/gc", &[]);
    r.folder("roms/gc/a", &["Zelda (Europe).gcm"]);
    r.folder("roms/gc/b", &["Zelda (USA).gcm"]);
    let output = r.run(&[&folder, "--emulator", "dolphin", "--game", "Zelda"]);
    failed_with(
        &output,
        1,
        "`Zelda` matches more than one game: a/Zelda (Europe).gcm, b/Zelda (USA).gcm. Pass one of those paths.",
    );
    let output = r.run(&[
        &folder,
        "--emulator",
        "dolphin",
        "--game",
        "a/Zelda (Europe).gcm",
    ]);
    assert_success(&output);
    assert!(r.config("zelda").contains("a/Zelda (Europe).gcm"));
    let full = Path::new(&folder).join("b/Zelda (USA).gcm");
    let output = r.run(&[
        &folder,
        "--emulator",
        "dolphin",
        "--game",
        full.to_str().unwrap(),
    ]);
    assert_success(&output);
    assert!(r.config("zelda-gamecube").contains("b/Zelda (USA).gcm"));
}

#[test]
fn a_single_file_refuses_all_and_another_game() {
    let r = Roms::new();
    r.emulator("mgba");
    let folder = r.folder("roms/gb", &["Tetris.gb"]);
    let rom = Path::new(&folder).join("Tetris.gb");
    let rom = rom.to_str().unwrap();
    let output = r.run(&[rom, "--emulator", "mgba", "--all", "--yes"]);
    failed_with(&output, 1, "--all needs a folder of games, not one game.");
    let output = r.run(&[rom, "--emulator", "mgba", "--game", "Mario"]);
    failed_with(&output, 1, &format!("--game `Mario` doesn't match {rom}."));
    let output = r.run(&[rom, "--emulator", "mgba", "--game", "Tetris"]);
    assert_success(&output);
}

#[test]
fn core_with_a_guided_emulator_offers_only_retroarch() {
    let r = Roms::new();
    r.emulator("dolphin-emu");
    let output = r.run(&["--core", "snes9x"]);
    failed_with(
        &output,
        1,
        "--core is only for RetroArch, and wineo found no RetroArch.",
    );
    r.emulator("retroarch");
    let mut pty = r.pty(&["--core", "snes9x"]);
    pty.expect("Which emulator?");
    pty.expect("1) RetroArch (retroarch)");
    pty.expect("Emulator: ");
    pty.esc();
    pty.expect("Import canceled.");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(130), "{output}");
    assert!(!output.contains("Dolphin"), "{output}");
}

#[test]
fn the_review_says_whether_art_will_be_fetched_and_quotes_the_command() {
    let r = Roms::new();
    let folder = gamecube(&r);
    let rom = Path::new(&folder).join("Zelda [!].gcm");
    let rom = rom.to_str().unwrap();
    let output = r.run(&[rom, "--emulator", "dolphin", "--art", "--dry-run"]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("  art       none\n"), "{out}");
    assert!(
        out.contains(&format!("  command   dolphin-emu -b -e '{rom}'\n")),
        "{out}"
    );
    let output = r
        .command()
        .env("WINEO_STEAMGRIDDB_KEY", "fake")
        .args([
            "import",
            "roms",
            rom,
            "--emulator",
            "dolphin",
            "--art",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert_success(&output);
    assert!(
        stdout(&output).contains("  art       SteamGridDB, top match\n"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn list_and_dry_run_in_a_terminal_name_the_missing_flag() {
    let r = Roms::new();
    let folder = gamecube(&r);
    let mut pty = r.pty(&[&folder, "--list"]);
    pty.expect("--list needs --emulator ID or --custom EXE");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(1), "{output}");
    let mut pty = r.pty(&[&folder, "--emulator", "dolphin", "--dry-run"]);
    pty.expect("--dry-run needs --system gamecube or --system wii");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(1), "{output}");
    assert!(!output.contains("needs a terminal"), "{output}");
    let mut pty = r.pty(&[
        &folder,
        "--emulator",
        "dolphin",
        "--system",
        "gamecube",
        "--dry-run",
    ]);
    pty.expect("--dry-run needs --game NAME or --all");
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(1), "{output}");
    assert!(!output.contains("or --list"), "{output}");
}

#[test]
fn no_folder_without_a_terminal_says_to_pass_one() {
    let r = Roms::new();
    r.emulator("dolphin-emu");
    let output = r.run(&["--emulator", "dolphin"]);
    failed_with(
        &output,
        1,
        "guided ROM import needs a terminal; pass a folder or file of games",
    );
}

#[test]
fn nothing_to_pick_is_an_error_that_says_what_is_there() {
    let r = Roms::new();
    r.emulator("dolphin-emu");
    let folder = r.folder("roms/gc", &["Zelda.gcm"]);
    non_utf8_file(&folder, b"Bad\xff.gcm");
    assert_success(&r.run(&[&folder, "--emulator", "dolphin", "--game", "Zelda"]));
    let output = r.run(&[&folder, "--emulator", "dolphin"]);
    failed_with(
        &output,
        1,
        "guided ROM import needs a terminal; pass --game NAME, --all, or --list",
    );
    let mut pty = r.pty(&[&folder, "--emulator", "dolphin"]);
    pty.expect(&format!(
        "No games to import in {folder}: 1 already imported, 1 can't be imported."
    ));
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(1), "{output}");
    let empty = r.folder("roms/empty", &[]);
    let mut pty = r.pty(&[&empty, "--emulator", "dolphin"]);
    pty.expect(&format!("No games found in {empty}."));
    let (status, output) = pty.finish();
    assert_eq!(status.code(), Some(1), "{output}");
}

#[test]
fn a_path_install_comes_before_the_flatpak() {
    let r = Roms::new();
    r.emulator("pcsx2-qt");
    let exports = r.sys.join("var/lib/flatpak/exports/bin");
    fs::create_dir_all(&exports).unwrap();
    write_script(exports.join("net.pcsx2.PCSX2"), FAKE_EMULATOR);
    let folder = r.folder("roms/ps2", &["Okami.iso"]);
    let output = r.run(&["--emulators", "--json"]);
    let json: Value = serde_json::from_str(&stdout(&output)).unwrap();
    let installs = json["emulators"][0]["installs"].as_array().unwrap();
    assert_eq!(installs.len(), 2, "{json}");
    assert_eq!(installs[1]["kind"], "flatpak");
    assert_success(&r.run(&[&folder, "--emulator", "pcsx2", "--all", "--yes"]));
    let config = r.config("okami");
    assert!(config.contains("exe = \"pcsx2-qt\""), "{config}");
}

#[test]
fn no_emulators_found_says_how_to_name_one() {
    let r = Roms::new();
    let output = r.run(&[]);
    failed_with(
        &output,
        1,
        "wineo found no emulators; pass --emulator ID --exe PATH or --custom EXE",
    );
}

#[test]
fn a_retroarch_dry_run_writes_no_quit_file() {
    let r = Roms::new();
    r.emulator("retroarch");
    r.core("snes9x", "Nintendo - SNES / SFC (Snes9x)", "smc|sfc");
    let folder = r.folder("roms/snes", &["Super Metroid.sfc"]);
    let rom = Path::new(&folder).join("Super Metroid.sfc");
    let output = r.run(&[
        rom.to_str().unwrap(),
        "--emulator",
        "retroarch",
        "--dry-run",
    ]);
    assert_success(&output);
    assert!(
        stdout(&output).contains("--appendconfig"),
        "{}",
        stdout(&output)
    );
    assert!(!r.root().join("data/wineo/retroarch-quit.cfg").exists());
    assert!(r.environments().is_empty());
}

#[test]
fn list_json_names_a_subfolder_it_couldnt_read() {
    use std::os::unix::fs::PermissionsExt;
    let r = Roms::new();
    let folder = gamecube(&r);
    let locked = Path::new(&folder).join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("Hidden.gcm"), "").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let output = r.run(&[&folder, "--emulator", "dolphin", "--list", "--json"]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_success(&output);
    let json: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(
        json["skipped"],
        serde_json::json!([locked.display().to_string()])
    );
}

#[test]
fn completion_offers_twin_cores_once() {
    let r = Roms::new();
    twin_cores(&r);
    assert_eq!(
        complete(&r, &["wineo", "import", "roms", "--core", ""]),
        ["snes9x"]
    );
}

#[test]
fn going_back_past_a_given_folder_forgets_the_system_too() {
    let r = Roms::new();
    r.emulator("dolphin-emu");
    r.emulator("mgba");
    let folder = r.folder("roms/mixed", &["Metroid Prime (USA).rvz", "Tetris.gb"]);
    // The folder is given, so the emulator is the only question before the system.
    let mut pty = r.pty(&[&folder]);
    pty.expect("Emulator: ");
    pty.send("1");
    pty.expect("System: ");
    pty.send("2");
    pty.expect("Game: ");
    pty.esc();
    pty.expect("System");
    pty.esc();
    pty.expect("Emulator: ");
    pty.send("2");
    pty.expect("Game: ");
    pty.send("1");
    pty.expect("Import it?");
    pty.send("");
    pty.expect("Imported tetris.");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
}

#[test]
fn a_new_folder_asks_the_system_afresh() {
    let r = Roms::new();
    r.emulator("dolphin-emu");
    let first = r.folder("roms/one", &["Metroid Prime (USA).rvz"]);
    let second = r.folder("roms/two", &["Pikmin (USA).rvz"]);
    let mut pty = r.pty(&["--emulator", "dolphin"]);
    pty.expect("Folder or file of games");
    pty.send(&first);
    pty.expect("System: ");
    pty.send("2");
    pty.expect("Game: ");
    pty.esc();
    pty.expect("System");
    pty.esc();
    pty.expect("Folder or file of games");
    pty.send(&second);
    // No default left over from the first folder's answer.
    pty.expect("System: ");
    pty.send("1");
    pty.expect("Game: ");
    pty.send("1");
    pty.expect("Import it?");
    pty.send("");
    pty.expect("Imported pikmin.");
    let (status, output) = pty.finish();
    assert!(status.success(), "{output}");
    assert!(r.config("pikmin").contains("tags = [\"gamecube\"]"));
}
