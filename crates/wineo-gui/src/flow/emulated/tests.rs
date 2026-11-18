use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tempfile::TempDir;
use wineo_core::emulator::Install;
use wineo_core::rom_import::Scan;

use super::*;
use crate::layout_probe::{Laid, lay_out};
use crate::test_script::write_script;

fn paths(root: &Path, portable: bool) -> Paths {
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    Paths::select(
        move |key| (key == "HOME").then(|| OsString::from(&home)),
        portable.then(|| root.join("portable")).as_deref(),
    )
    .unwrap()
}

/// A host with fake `dolphin-emu`, `retroarch`, and `mgba-qt` on its PATH,
/// RetroArch's user cores, and empty system folders. Nothing reads the
/// real host.
fn host(root: &Path) -> Host {
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for name in ["dolphin-emu", "retroarch", "mgba-qt"] {
        write_script(bin.join(name), "#!/bin/sh\nexit 0\n");
    }
    let system_root = root.join("system");
    fs::create_dir_all(&system_root).unwrap();
    Host {
        path_var: bin.into_os_string(),
        exports: vec![root.join("exports")],
        system_root,
    }
}

fn core(dir: &Path, id: &str, name: &str, systemid: &str, extensions: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join(format!("{id}_libretro.so")), "").unwrap();
    fs::write(
        dir.join(format!("{id}_libretro.info")),
        format!(
            "display_name = \"{name}\"\nsystemid = \"{systemid}\"\nsupported_extensions = \"{extensions}\"\n"
        ),
    )
    .unwrap();
}

fn user_cores(root: &Path) -> PathBuf {
    root.join("home/.config/retroarch/cores")
}

/// Empty games in `root/roms`, by file name.
fn roms(root: &Path, files: &[&str]) -> PathBuf {
    let dir = root.join("roms");
    fs::create_dir_all(&dir).unwrap();
    for file in files {
        fs::write(dir.join(file), "").unwrap();
    }
    dir
}

fn open(paths: &Paths, host: &Host) -> Page {
    let (mut page, _) = Page::with_host(paths.clone(), host.clone());
    assert!(page.waiting());
    let _ = page.update(Event::Found(find(paths, Some(host.clone()))), false);
    page
}

/// The emulator page's row for `emulator`'s PATH install.
fn install_row(page: &Page, emulator: Emulator) -> usize {
    page.found
        .as_ref()
        .unwrap()
        .installs
        .iter()
        .position(|(found, _)| *found == emulator)
        .unwrap()
}

/// Runs the work the page is waiting for, as its tasks would, until it
/// waits for nothing.
fn settle(page: &mut Page) {
    loop {
        let generation = page.generation;
        let event = match page.status.as_str() {
            "Looking for RetroArch cores..." => {
                let host = page.host().unwrap().clone();
                let install = page.chosen.as_ref().unwrap().retroarch().unwrap().clone();
                let folder = page.listed_folder.clone().unwrap();
                Event::Cores(
                    generation,
                    work::matching_cores(&page.paths, &host, &install, &folder),
                )
            }
            "Looking for games..." => {
                let scan = page.chosen.as_ref().unwrap().scan(page.core.as_ref());
                let folder = page.listed_folder.clone().unwrap();
                Event::Listed(generation, work::listing(&page.paths, &scan, &folder))
            }
            "Checking games..." => {
                let (emulating, entries, system) = page.plan_inputs().unwrap();
                Event::Planned(
                    generation,
                    work::plan(&page.paths, &emulating, entries, system.as_deref()),
                )
            }
            _ => return,
        };
        let _ = page.update(event, false);
    }
}

/// Picks `emulator`'s install and the folder, and lists what's there.
fn to_folder(page: &mut Page, emulator: Emulator, folder: &Path) {
    let row = install_row(page, emulator);
    let _ = page.update(Event::Install(row), false);
    assert_eq!(page.step, Step::Folder);
    let _ = page.update(Event::Folder(folder.display().to_string()), false);
    let _ = page.update(Event::Next, false);
    settle(page);
}

/// Runs `task` to its end as the runtime would, answering its widget
/// operations on `laid` when given, and returns what it sent.
fn drive(laid: Option<&mut Laid<'_, Event>>, task: Task<Event>) -> Vec<Event> {
    use iced::futures::StreamExt;
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return Vec::new();
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut laid = laid;
    let mut sent = Vec::new();
    runtime.block_on(async {
        // Generous, since the import's work runs on the blocking pool.
        while let Ok(Some(action)) =
            tokio::time::timeout(std::time::Duration::from_secs(20), stream.next()).await
        {
            match action {
                iced_runtime::Action::Output(event) => sent.push(event),
                iced_runtime::Action::Widget(operation) => {
                    laid.as_mut().expect("a laid out page").run(operation)
                }
                _ => {}
            }
        }
    });
    sent
}

/// Each text the page shows, in tree order.
fn texts(page: &Page, key_available: bool) -> Vec<String> {
    use iced::advanced::widget::Operation;
    struct Texts(Arc<Mutex<Vec<String>>>);
    impl Operation for Texts {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn text(&mut self, _id: Option<&iced::widget::Id>, _bounds: iced::Rectangle, text: &str) {
            self.0.lock().unwrap().push(text.to_owned());
        }
    }
    let found = Arc::new(Mutex::new(Vec::new()));
    let mut laid = lay_out(
        page.view(crate::theme::Palette::DARK, 900.0, key_available),
        iced::Size::new(900.0, 720.0),
    );
    laid.run(Box::new(Texts(found.clone())));
    found.lock().unwrap().clone()
}

/// Which focusable has focus, by its place among them all, and how many
/// there are.
fn focus_place(laid: &mut Laid<'_, Event>) -> (Option<usize>, usize) {
    use iced::advanced::widget::Operation;
    use iced::advanced::widget::operation::Focusable;
    struct Place(Arc<Mutex<(Option<usize>, usize)>>);
    impl Operation for Place {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn focusable(
            &mut self,
            _id: Option<&iced::widget::Id>,
            _bounds: iced::Rectangle,
            state: &mut dyn Focusable,
        ) {
            let mut place = self.0.lock().unwrap();
            if state.is_focused() {
                place.0 = Some(place.1);
            }
            place.1 += 1;
        }
    }
    let found = Arc::new(Mutex::new((None, 0)));
    laid.run(Box::new(Place(found.clone())));
    *found.lock().unwrap()
}

fn titles(page: &Page) -> Vec<&str> {
    page.listing
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .map(|entry| entry.title.as_str())
        .collect()
}

#[test]
fn the_emulator_page_lists_found_installs_then_other_and_custom() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let page = open(&paths, &host);
    assert_eq!(page.step, Step::Emulator);
    assert!(!page.waiting());
    let found: Vec<Emulator> = page
        .found
        .as_ref()
        .unwrap()
        .installs
        .iter()
        .map(|(emulator, _)| *emulator)
        .collect();
    // Table order, PATH installs only, since the exports folder is empty.
    assert_eq!(
        found,
        [Emulator::Dolphin, Emulator::RetroArch, Emulator::Mgba]
    );
    let rows = page.rows().unwrap();
    assert_eq!(rows.list, EMULATOR_LIST);
    assert_eq!(rows.ids.len(), 5);
    assert!(rows.ids[0].starts_with("emulated-install:dolphin:"));
    assert_eq!(rows.ids[3..], [OTHER_ROW, CUSTOM_ROW]);
    assert_eq!(rows.focus, None);
    assert_eq!(
        work::install_label(
            Emulator::Dolphin,
            &page.found.as_ref().unwrap().installs[0].1
        ),
        "Dolphin (dolphin-emu)"
    );
}

#[test]
fn a_portable_root_is_refused_with_no_emulators_listed() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), true);
    let host = host(temp.path());
    let page = open(&paths, &host);
    assert_eq!(page.error.as_deref(), Some(work::PORTABLE_REFUSAL));
    assert!(page.found.is_none());
    assert!(page.rows().is_none());
    assert_eq!(
        work::PORTABLE_REFUSAL,
        "Emulated games can't be imported into a portable folder."
    );
}

#[test]
fn back_walks_the_pages_in_order_and_lands_on_the_row_it_came_from() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let folder = roms(temp.path(), &["Metroid Prime (USA).rvz", "Zelda.gcm"]);
    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::Dolphin, &folder);
    // An .rvz could be either system, so the system page asks.
    assert_eq!(page.step, Step::System);
    let rows = page.rows().unwrap();
    assert_eq!(rows.list, SYSTEM_LIST);
    assert_eq!(
        rows.ids,
        ["emulated-system:gamecube", "emulated-system:wii"]
    );
    let _ = page.update(Event::System(1), false);
    assert_eq!(page.step, Step::Games);
    assert_eq!(page.system.as_deref(), Some("wii"));

    let _ = page.update(Event::Back, false);
    assert_eq!(page.step, Step::System);
    assert_eq!(
        page.rows().unwrap().focus.as_deref(),
        Some("emulated-system:wii")
    );
    let _ = page.update(Event::Back, false);
    assert_eq!(page.step, Step::Folder);
    assert!(page.rows().is_none());
    let _ = page.update(Event::Back, false);
    assert_eq!(page.step, Step::Emulator);
    let rows = page.rows().unwrap();
    assert_eq!(rows.focus.as_deref(), Some(rows.ids[0].as_str()));
    let _ = page.update(Event::Back, false);
    assert!(matches!(page.take_action(), Some(Action::Choose)));
}

#[test]
fn other_asks_which_built_in_the_file_is() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let app = temp.path().join("apps/Dolphin.AppImage");
    fs::create_dir_all(app.parent().unwrap()).unwrap();
    write_script(&app, "#!/bin/sh\nexit 0\n");
    let mut page = open(&paths, &host);
    let _ = page.update(Event::Other, false);
    assert_eq!(page.step, Step::Other);
    assert!(page.rows().is_none());

    let _ = page.update(Event::OtherFile(app.display().to_string()), false);
    let _ = page.update(Event::Next, false);
    assert_eq!(page.step, Step::Other);
    assert_eq!(page.error.as_deref(), Some("Choose which emulator it is."));

    let _ = page.update(Event::OtherFile("~/Apps/Game.exe".into()), false);
    let _ = page.update(
        Event::OtherEmulator(EmulatorChoice(Emulator::Dolphin)),
        false,
    );
    let _ = page.update(Event::Next, false);
    assert_eq!(page.step, Step::Other);
    assert!(page.error.as_deref().unwrap().contains("Windows program"));

    let _ = page.update(Event::OtherFile(app.display().to_string()), false);
    let _ = page.update(Event::Next, false);
    assert_eq!(page.step, Step::Folder);
    assert_eq!(
        page.chosen,
        Some(Chosen::BuiltIn(
            Emulator::Dolphin,
            Install::File(app.clone())
        ))
    );
    // Back returns to the file page with what was typed.
    let _ = page.update(Event::Back, false);
    assert_eq!(page.step, Step::Other);
    assert_eq!(page.other_file, app.display().to_string());
    let _ = page.update(Event::Back, false);
    assert_eq!(
        page.rows().unwrap().focus.as_deref(),
        Some(OTHER_ROW),
        "Back lands on Other..."
    );
}

#[test]
fn custom_takes_a_command_arguments_file_types_and_a_system() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let exe = temp.path().join("apps/eden");
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    write_script(&exe, "#!/bin/sh\nexit 0\n");
    let folder = roms(temp.path(), &["Hollow Knight.nsp", "notes.txt"]);
    let mut page = open(&paths, &host);
    let _ = page.update(Event::Custom, false);
    assert_eq!(page.step, Step::Custom);

    let _ = page.update(Event::Next, false);
    assert_eq!(
        page.error.as_deref(),
        Some("The emulator's command is empty.")
    );
    let _ = page.update(Event::CustomExe(exe.display().to_string()), false);
    let _ = page.update(Event::CustomArgs("-f -g".into()), false);
    let _ = page.update(Event::Next, false);
    assert!(
        page.error.as_deref().unwrap().contains("{rom}"),
        "{:?}",
        page.error
    );
    let _ = page.update(Event::CustomArgs("-f -g {rom}".into()), false);
    let _ = page.update(Event::Next, false);
    assert_eq!(
        page.error.as_deref(),
        Some("Give at least one file type for the emulator's games.")
    );
    let _ = page.update(Event::CustomExtensions(".NSP, xci".into()), false);
    let _ = page.update(Event::CustomSystem("Switch".into()), false);
    let _ = page.update(Event::Next, false);
    assert_eq!(page.step, Step::Folder, "{:?}", page.error);
    assert!(!page.chosen.as_ref().unwrap().has_fullscreen());

    let _ = page.update(Event::Folder(folder.display().to_string()), false);
    let _ = page.update(Event::Next, false);
    settle(&mut page);
    assert_eq!(page.step, Step::Games);
    assert_eq!(titles(&page), ["Hollow Knight"]);
    let _ = page.update(Event::SelectAll, false);
    let _ = page.update(Event::ImportSelected, false);
    settle(&mut page);
    assert_eq!(page.step, Step::Review);
    let review = page.review.as_ref().unwrap();
    assert_eq!(review.ready.len(), 1);
    let plan = &review.ready[0];
    assert_eq!(plan.emulator, "custom");
    assert_eq!(plan.system.as_deref(), Some("switch"));
    assert_eq!(plan.target.args[..2], ["-f", "-g"]);

    // Back from the folder returns to Custom with the fields kept.
    for _ in 0..3 {
        let _ = page.update(Event::Back, false);
    }
    assert_eq!(page.step, Step::Custom);
    assert_eq!(page.custom.args, "-f -g {rom}");
}

#[test]
fn retroarchs_core_page_offers_only_the_cores_that_play_the_folder() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let cores = user_cores(temp.path());
    core(&cores, "snes9x", "Snes9x", "super_nes", "smc|sfc");
    core(&cores, "bsnes", "bsnes", "super_nes", "sfc");
    core(&cores, "mgba", "mGBA", "game_boy_advance", "gba");
    let folder = roms(temp.path(), &["Super Metroid (Japan, USA).sfc"]);
    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::RetroArch, &folder);
    assert_eq!(page.step, Step::Core);
    let rows = page.rows().unwrap();
    assert_eq!(rows.list, CORE_LIST);
    let offered: Vec<&str> = page.cores.as_ref().unwrap().iter().map(Core::id).collect();
    assert_eq!(offered, ["bsnes", "snes9x"]);
    assert_eq!(rows.ids.len(), 2);

    let _ = page.update(Event::Core(1), false);
    settle(&mut page);
    assert_eq!(page.step, Step::Games);
    assert_eq!(page.core.as_ref().unwrap().id(), "snes9x");
    assert_eq!(titles(&page), ["Super Metroid"]);
    let _ = page.update(Event::Back, false);
    assert_eq!(page.step, Step::Core);
    assert!(
        page.rows()
            .unwrap()
            .focus
            .unwrap()
            .ends_with("snes9x_libretro.so")
    );

    // With one matching core there's nothing to ask.
    let gba = roms(&temp.path().join("gba"), &["Metroid Fusion.gba"]);
    let _ = page.update(Event::Back, false);
    let _ = page.update(Event::Folder(gba.display().to_string()), false);
    let _ = page.update(Event::Next, false);
    settle(&mut page);
    assert_eq!(page.step, Step::Games);
    assert_eq!(page.core.as_ref().unwrap().id(), "mgba");
    let _ = page.update(Event::Back, false);
    assert_eq!(page.step, Step::Folder, "the core page was skipped");
}

#[test]
fn duplicate_cores_are_told_apart_by_folder() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let distro = host.system_root.join("usr/lib/libretro");
    core(&distro, "snes9x", "Snes9x", "super_nes", "sfc");
    core(
        &user_cores(temp.path()),
        "snes9x",
        "Snes9x",
        "super_nes",
        "sfc",
    );
    let folder = roms(temp.path(), &["Super Metroid.sfc"]);
    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::RetroArch, &folder);
    assert_eq!(page.step, Step::Core);
    let cores = page.cores.clone().unwrap();
    let labels: Vec<String> = cores
        .iter()
        .map(|core| work::core_label(core, &cores))
        .collect();
    assert_eq!(labels.len(), 2);
    assert_ne!(labels[0], labels[1]);
    assert!(
        labels
            .iter()
            .any(|label| label == &format!("Snes9x (snes9x) in {}", distro.display()))
    );
    let rows = page.rows().unwrap();
    assert_ne!(rows.ids[0], rows.ids[1]);
    // A core alone keeps its plain label.
    assert_eq!(work::core_label(&cores[0], &cores[..1]), "Snes9x (snes9x)");
}

#[test]
fn changing_the_emulator_clears_the_core_system_and_games() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let cores = user_cores(temp.path());
    core(&cores, "snes9x", "Snes9x", "super_nes", "sfc|rvz");
    core(&cores, "bsnes", "bsnes", "super_nes", "sfc|rvz");
    let folder = roms(temp.path(), &["Metroid Prime.rvz", "Super Metroid.sfc"]);
    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::RetroArch, &folder);
    let _ = page.update(Event::Core(0), false);
    settle(&mut page);
    assert_eq!(page.step, Step::Games);
    let _ = page.update(Event::SelectAll, false);
    assert!(page.core.is_some() && !page.grid.chosen.is_empty());

    // Back to the emulator page, then the same emulator again keeps it all.
    for _ in 0..3 {
        let _ = page.update(Event::Back, false);
    }
    assert_eq!(page.step, Step::Emulator);
    let _ = page.update(
        Event::Install(install_row(&page, Emulator::RetroArch)),
        false,
    );
    assert!(page.core.is_some() && !page.grid.chosen.is_empty());

    let _ = page.update(Event::Back, false);
    let _ = page.update(Event::Install(install_row(&page, Emulator::Dolphin)), false);
    assert!(page.core.is_none());
    assert!(page.cores.is_none());
    assert!(page.system.is_none());
    assert!(page.listing.is_none());
    assert!(page.grid.chosen.is_empty());
    let _ = page.update(Event::Next, false);
    settle(&mut page);
    assert_eq!(page.step, Step::System, "Dolphin asks for its .rvz");
    let _ = page.update(Event::System(0), false);
    assert_eq!(page.system.as_deref(), Some("gamecube"));

    // A new folder clears the system too.
    let other = roms(&temp.path().join("other"), &["Wii Sports.rvz"]);
    for _ in 0..2 {
        let _ = page.update(Event::Back, false);
    }
    assert_eq!(page.step, Step::Folder);
    let _ = page.update(Event::Folder(other.display().to_string()), false);
    let _ = page.update(Event::Next, false);
    assert!(page.system.is_none());
}

#[test]
fn tiles_show_already_imported_games_and_file_names_for_one_title() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let folder = roms(
        temp.path(),
        &[
            "Pokemon Emerald (USA).gba",
            "Pokemon Emerald (Europe).gba",
            "Metroid Fusion.gba",
        ],
    );
    let config = paths.environment_config("metroid-fusion");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        format!(
            "schema_version = 1\nkind = \"native\"\n[emulation]\nemulator = \"mgba\"\nrom = \"{}\"\n[targets.metroid-fusion]\nexe = \"mgba-qt\"\n",
            folder.join("Metroid Fusion.gba").display()
        ),
    )
    .unwrap();
    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::Mgba, &folder);
    assert_eq!(page.step, Step::Games);
    let entries = page.listing.as_ref().unwrap().entries.clone();
    assert_eq!(
        titles(&page),
        ["Metroid Fusion", "Pokemon Emerald", "Pokemon Emerald"]
    );
    assert_eq!(
        entries[0].state,
        EntryState::AlreadyImported("metroid-fusion".into())
    );
    let notes: Vec<Option<&str>> = entries.iter().map(|entry| entry.note.as_deref()).collect();
    assert_eq!(
        notes,
        [
            None,
            Some("Pokemon Emerald (Europe).gba"),
            Some("Pokemon Emerald (USA).gba")
        ]
    );
    // The imported one can't be picked, by click or by Select all.
    let _ = page.update(Event::Toggle(0), false);
    assert!(page.grid.chosen.is_empty());
    let _ = page.update(Event::SelectAll, false);
    assert_eq!(page.grid.chosen.len(), 2);
    let _ = page.update(Event::Toggle(1), false);
    assert_eq!(page.grid.chosen.len(), 1);
}

#[test]
fn the_fullscreen_option_is_absent_for_dolphin_and_drops_the_flag_when_off() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let folder = roms(temp.path(), &["Zelda.gcm", "Metroid Fusion.gba"]);
    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::Dolphin, &folder);
    assert!(!page.chosen.as_ref().unwrap().has_fullscreen());
    let _ = page.update(Event::SelectAll, false);
    let _ = page.update(Event::ImportSelected, false);
    settle(&mut page);
    assert_eq!(page.step, Step::Review);
    let shown = texts(&page, false);
    assert!(
        shown.iter().any(|text| text == "Source: Dolphin"),
        "{shown:?}"
    );
    assert!(
        !shown.iter().any(|text| text.starts_with("Fullscreen")),
        "{shown:?}"
    );

    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::Mgba, &folder);
    assert!(page.chosen.as_ref().unwrap().has_fullscreen());
    assert!(page.fullscreen, "on by default");
    let _ = page.update(Event::SelectAll, false);
    let _ = page.update(Event::ImportSelected, false);
    settle(&mut page);
    assert!(
        texts(&page, false)
            .iter()
            .any(|text| text == "Fullscreen: on")
    );
    let on = page.review.as_ref().unwrap().ready[0].target.args.clone();
    let _ = page.update(Event::Back, false);
    let _ = page.update(Event::Fullscreen(false), false);
    let _ = page.update(Event::ImportSelected, false);
    settle(&mut page);
    let off = page.review.as_ref().unwrap().ready[0].target.args.clone();
    assert!(
        texts(&page, false)
            .iter()
            .any(|text| text == "Fullscreen: off")
    );
    assert_eq!(on.len(), off.len() + 1, "{on:?} {off:?}");
    assert!(on.contains(&"-f".to_owned()) && !off.contains(&"-f".to_owned()));
}

#[test]
fn the_art_checkbox_needs_a_key_and_art_to_fetch() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let folder = roms(temp.path(), &["Metroid Fusion.gba"]);
    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::Mgba, &folder);
    assert!(!page.fetch_art, "off by default");
    let _ = page.update(Event::FetchArt(true), false);
    assert!(!page.fetch_art, "no key");
    let _ = page.update(Event::FetchArt(true), true);
    assert!(page.fetch_art);

    fs::create_dir_all(paths.global_config().parent().unwrap()).unwrap();
    fs::write(paths.global_config(), "[art]\nfetch = \"none\"\n").unwrap();
    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::Mgba, &folder);
    let _ = page.update(Event::FetchArt(true), true);
    assert!(!page.fetch_art, "every kind is off");
}

#[test]
fn a_finished_import_adds_play_cards_with_the_emulator_as_source_and_their_art() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let folder = roms(temp.path(), &["Zelda (USA).gcm", "Metroid Prime.wbfs"]);
    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::Dolphin, &folder);
    assert_eq!(page.step, Step::Games);
    let _ = page.update(Event::SelectAll, false);
    let _ = page.update(Event::FetchArt(true), true);
    let _ = page.update(Event::ImportSelected, true);
    assert!(page.fetch_art);
    settle(&mut page);
    assert_eq!(page.step, Step::Review);

    let fetched: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let seen = fetched.clone();
    let fetcher: Fetcher = Arc::new(move |_: &Paths, name: &str, title: &str, _: &Token| {
        seen.lock()
            .unwrap()
            .push((name.to_owned(), title.to_owned()));
        Ok(vec!["No artwork match".to_owned()])
    });
    page.fetcher = Some(fetcher);
    let task = page.update(Event::Import, true);
    assert_eq!(page.step, Step::Working);
    assert!(page.busy());
    let mut progress = Vec::new();
    for event in drive(None, task) {
        if let Event::Progress(_, line) = &event {
            progress.push(line.clone());
        }
        let _ = page.update(event, true);
    }
    assert_eq!(
        progress,
        [
            "(1 of 2) Importing Metroid Prime...",
            "(2 of 2) Importing Zelda..."
        ]
    );
    assert_eq!(page.step, Step::Finished);
    assert!(!page.busy());
    let report = page.report.as_ref().unwrap();
    assert_eq!(work::report_lines(report), ["Imported 2 of 2 games."]);
    assert_eq!(
        report.warnings,
        ["metroid-prime: No artwork match", "zelda: No artwork match"]
    );
    assert_eq!(
        *fetched.lock().unwrap(),
        [
            ("metroid-prime".to_owned(), "Metroid Prime".to_owned()),
            ("zelda".to_owned(), "Zelda".to_owned())
        ]
    );

    let cards = crate::library::load_library(&paths).unwrap();
    for (name, tag) in [("metroid-prime", "wii"), ("zelda", "gamecube")] {
        let card = cards.iter().find(|card| card.name == name).unwrap();
        assert_eq!(card.source.label(), Some("Dolphin"), "{name}");
        assert_eq!(card.launch_button_label(), "Play", "{name}");
        assert_eq!(card.tags, [tag], "{name}");
    }
    let _ = page.update(Event::BackToLibrary, true);
    assert!(matches!(
        page.take_action(),
        Some(Action::Library { name: None, .. })
    ));
}

#[test]
fn a_rerun_lists_the_imported_games_as_already_imported() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let folder = roms(temp.path(), &["Metroid Fusion.gba", "Zelda.gba"]);
    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::Mgba, &folder);
    let _ = page.update(Event::Toggle(0), false);
    let _ = page.update(Event::ImportSelected, false);
    settle(&mut page);
    let report = work::run_import(
        &paths,
        page.review.clone().unwrap(),
        &Token::new(),
        None,
        |_| {},
    )
    .unwrap();
    assert_eq!(report.done, ["metroid-fusion"]);

    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::Mgba, &folder);
    let states: Vec<EntryState> = page
        .listing
        .unwrap()
        .entries
        .into_iter()
        .map(|entry| entry.state)
        .collect();
    assert_eq!(
        states,
        [
            EntryState::AlreadyImported("metroid-fusion".into()),
            EntryState::New
        ]
    );
}

#[test]
fn a_missing_folder_or_one_without_cores_says_why() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let folder = roms(temp.path(), &["Super Metroid.sfc"]);
    let mut page = open(&paths, &host);
    let _ = page.update(
        Event::Install(install_row(&page, Emulator::RetroArch)),
        false,
    );
    let _ = page.update(Event::Next, false);
    assert_eq!(
        page.error.as_deref(),
        Some("Enter a folder or file of games.")
    );
    let missing = temp.path().join("nowhere");
    let _ = page.update(Event::Folder(missing.display().to_string()), false);
    let _ = page.update(Event::Next, false);
    assert_eq!(
        page.error,
        Some(format!("{} doesn't exist.", missing.display()))
    );
    let _ = page.update(Event::Folder(folder.display().to_string()), false);
    let _ = page.update(Event::Next, false);
    settle(&mut page);
    assert_eq!(page.step, Step::Folder);
    assert_eq!(
        page.error.as_deref(),
        Some("No RetroArch cores were found.")
    );
    core(
        &user_cores(temp.path()),
        "mgba",
        "mGBA",
        "game_boy_advance",
        "gba",
    );
    let _ = page.update(Event::Next, false);
    settle(&mut page);
    assert_eq!(
        page.error,
        Some(format!(
            "No installed RetroArch core plays the files in {}.",
            folder.display()
        ))
    );
}

#[test]
fn key_6_opens_the_emulator_page_and_the_arrows_walk_its_rows() {
    use crate::focus_button::{landed, walk_rows};
    use crate::{Message, flow};
    use iced::keyboard::{Modifiers, key::Named};
    let temp = TempDir::new().unwrap();
    let host = host(temp.path());
    let mut app = crate::tests::app();
    let _ = app.update(Message::Add);
    let _ = app.update(Message::Key(
        Key::Character("6".into()),
        Modifiers::empty(),
        iced::event::Status::Ignored,
    ));
    assert!(matches!(app.flow, Some(flow::Page::Emulated(_))));
    assert!(app.flow.as_ref().unwrap().rows().is_none(), "still looking");
    let found = find(&app.paths, Some(host));
    let task = app.update(Message::Flow(
        app.flow_generation,
        flow::Event::from(Event::Found(found)),
    ));
    let ids = app.flow.as_ref().unwrap().rows().unwrap().ids;
    let mut laid = landed(&app, task);
    assert_eq!(laid.focused(), [iced::widget::Id::from(ids[0].clone())]);
    walk_rows(&mut laid, &ids);
    // The walk ends back on the first row, Dolphin's, which Enter picks.
    let (published, _) = crate::focus_button::press(&mut laid, Named::Enter, Modifiers::empty());
    assert_eq!(published.len(), 1, "{published:?}");
    drop(laid);
    for message in published {
        let _ = app.update(message);
    }
    assert!(matches!(
        &app.flow,
        Some(flow::Page::Emulated(page))
            if page.step == Step::Folder
                && matches!(page.chosen, Some(Chosen::BuiltIn(Emulator::Dolphin, _)))
    ));
}

#[test]
fn the_core_and_system_rows_take_focus_in_order() {
    use crate::focus_button::walk_rows;
    use crate::layout_probe::lay_out;
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let cores = user_cores(temp.path());
    core(&cores, "snes9x", "Snes9x", "super_nes", "sfc");
    core(&cores, "bsnes", "bsnes", "super_nes", "sfc");
    let folder = roms(temp.path(), &["Super Metroid.sfc", "Zelda.rvz"]);
    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::RetroArch, &folder);
    assert_eq!(page.step, Step::Core);
    let ids = page.rows().unwrap().ids;
    let mut laid = lay_out(
        page.view(crate::theme::Palette::DARK, 900.0, false),
        iced::Size::new(900.0, 720.0),
    );
    walk_rows(&mut laid, &ids);
    drop(laid);

    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::Dolphin, &folder);
    assert_eq!(page.step, Step::System);
    let ids = page.rows().unwrap().ids;
    let mut laid = lay_out(
        page.view(crate::theme::Palette::DARK, 900.0, false),
        iced::Size::new(900.0, 720.0),
    );
    walk_rows(&mut laid, &ids);
}

#[test]
fn every_step_sits_inside_the_dialog_padding() {
    use crate::layout_probe::{bounds_at, probe, within};
    use wineo_core::rom_import::{Already, Failure};
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let cores = user_cores(temp.path());
    core(&cores, "snes9x", "Snes9x", "super_nes", "sfc");
    core(&cores, "bsnes", "bsnes", "super_nes", "sfc");
    let files: Vec<String> = (0..30)
        .map(|index| format!("The Legend of a Very Long Game Title Number {index} (USA).sfc"))
        .collect();
    let files: Vec<&str> = files.iter().map(String::as_str).collect();
    let folder = roms(temp.path(), &files);
    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::RetroArch, &folder);
    let _ = page.update(Event::Core(0), false);
    settle(&mut page);
    let _ = page.update(Event::SelectAll, false);
    let _ = page.update(Event::ImportSelected, false);
    settle(&mut page);
    let mut review = page.review.clone().unwrap();
    let failure = Failure {
        rom: folder.join(files[0]),
        title: "The Legend of a Very Long Game Title".to_owned(),
        name: None,
        reason: "The emulator's arguments have an unclosed quote or a trailing backslash."
            .to_owned(),
    };
    review.already.push(Already {
        rom: folder.join(files[1]),
        title: "Zelda".to_owned(),
        name: "zelda".to_owned(),
    });
    review.failures.push(failure.clone());
    page.review = Some(review);
    page.report = Some(BatchReport {
        done: vec!["zelda".to_owned()],
        total: 3,
        already: vec!["metroid".to_owned()],
        failures: vec![failure],
        warnings: vec!["zelda: Artwork: SteamGridDB couldn't be reached".to_owned()],
        canceled: true,
    });
    page.error = Some("No installed RetroArch core plays the files in the folder.".to_owned());
    for (width, height) in [(280.0, 240.0), (360.0, 280.0), (900.0, 720.0)] {
        let inset = iced::Rectangle::new(
            iced::Point::new(24.0, 24.0),
            iced::Size::new(width - 48.0, height - 48.0),
        );
        for step in [
            Step::Emulator,
            Step::Other,
            Step::Custom,
            Step::Folder,
            Step::Core,
            Step::System,
            Step::Games,
            Step::Review,
            Step::Working,
            Step::Finished,
        ] {
            page.step = step;
            page.token = (step == Step::Working).then(Token::new);
            let mut view = page.view(crate::theme::Palette::DARK, width, true);
            let probe = probe(&mut view, iced::Size::new(width, height));
            let spilled = probe.outside(inset);
            assert_eq!(probe.scrollables.len(), 1, "{step:?}: one scrolling region");
            assert!(
                spilled.is_empty(),
                "{step:?} at {width}x{height}: {spilled:?}"
            );
            let back = bounds_at(&probe.node, &[0, 0, 0]);
            assert!(within(back, inset), "{step:?}: {back:?}");
            if matches!(
                step,
                Step::Other
                    | Step::Custom
                    | Step::Folder
                    | Step::Review
                    | Step::Working
                    | Step::Finished
            ) {
                crate::layout_probe::assert_back_on_top_and_forward_pinned(
                    &probe,
                    inset,
                    &format!("{step:?} at {width}x{height}"),
                );
            }
        }
    }
}

/// Lays out the page, focuses the row `id`, presses Ctrl+Enter, and hands
/// the page what that sent.
fn ctrl_enter_on(page: &mut Page, id: Option<&str>) {
    use crate::layout_probe::relay_out;
    let size = iced::Size::new(900.0, 720.0);
    let mut laid = lay_out(page.view(crate::theme::Palette::DARK, 900.0, false), size);
    if let Some(id) = id {
        laid.run(Box::new(
            iced::advanced::widget::operation::focusable::focus::<()>(iced::widget::Id::from(
                id.to_owned(),
            )),
        ));
        assert_eq!(laid.focused(), [iced::widget::Id::from(id.to_owned())]);
    }
    let tree = laid.into_tree();
    let task = page.submit(false);
    let mut laid = relay_out(
        page.view(crate::theme::Palette::DARK, 900.0, false),
        tree,
        size,
    );
    let sent = drive(Some(&mut laid), task);
    drop(laid);
    for event in sent {
        let _ = page.update(event, false);
    }
}

#[test]
fn ctrl_enter_opens_the_focused_row_on_the_emulator_core_and_system_pages() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let cores = user_cores(temp.path());
    core(&cores, "snes9x", "Snes9x", "super_nes", "sfc");
    core(&cores, "bsnes", "bsnes", "super_nes", "sfc");
    let folder = roms(temp.path(), &["Super Metroid.sfc", "Zelda.rvz"]);

    let mut page = open(&paths, &host);
    ctrl_enter_on(&mut page, None);
    assert_eq!(page.step, Step::Emulator, "no row focused, nothing opens");
    let ids = page.rows().unwrap().ids;
    ctrl_enter_on(&mut page, Some(&ids[1]));
    assert_eq!(page.step, Step::Folder);
    assert!(matches!(
        page.chosen,
        Some(Chosen::BuiltIn(Emulator::RetroArch, _))
    ));
    let _ = page.update(Event::Back, false);
    ctrl_enter_on(&mut page, Some(CUSTOM_ROW));
    assert_eq!(page.step, Step::Custom);
    let _ = page.update(Event::Back, false);

    let _ = page.update(
        Event::Install(install_row(&page, Emulator::RetroArch)),
        false,
    );
    let _ = page.update(Event::Folder(folder.display().to_string()), false);
    let _ = page.update(Event::Next, false);
    settle(&mut page);
    assert_eq!(page.step, Step::Core);
    let ids = page.rows().unwrap().ids;
    ctrl_enter_on(&mut page, Some(&ids[1]));
    settle(&mut page);
    assert_eq!(page.step, Step::Games);
    assert_eq!(page.core.as_ref().unwrap().id(), "snes9x");

    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::Dolphin, &folder);
    assert_eq!(page.step, Step::System);
    ctrl_enter_on(&mut page, Some("emulated-system:wii"));
    assert_eq!(page.step, Step::Games);
    assert_eq!(page.system.as_deref(), Some("wii"));
}

/// Presses Tab, or Shift+Tab, as the flow's keys do, and says where focus
/// went.
fn tab(laid: &mut Laid<'_, Event>, shift: bool) -> Option<usize> {
    let walk = if shift {
        iced::widget::operation::focus_previous::<Event>()
    } else {
        iced::widget::operation::focus_next::<Event>()
    };
    crate::focus_button::run_task(laid, walk);
    focus_place(laid).0
}

#[test]
fn tab_and_shift_tab_walk_the_other_custom_and_folder_pages() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    // Only the fields take Tab: the buttons and the dropdown don't, as on
    // the other flows' pages.
    for (step, count) in [(Step::Other, 1), (Step::Custom, 4), (Step::Folder, 1)] {
        let mut page = open(&paths, &host);
        match step {
            Step::Other => {
                let _ = page.update(Event::Other, false);
            }
            Step::Custom => {
                let _ = page.update(Event::Custom, false);
            }
            _ => {
                let _ = page.update(Event::Install(0), false);
            }
        }
        assert_eq!(page.step, step);
        let mut laid = lay_out(
            page.view(crate::theme::Palette::DARK, 900.0, false),
            iced::Size::new(900.0, 720.0),
        );
        assert_eq!(focus_place(&mut laid), (None, count), "{step:?}");
        for place in 0..count {
            assert_eq!(tab(&mut laid, false), Some(place), "{step:?}");
        }
        assert_eq!(tab(&mut laid, false), None, "{step:?}: Tab lets go");
        for place in (0..count).rev() {
            assert_eq!(tab(&mut laid, true), Some(place), "{step:?}");
        }
    }
}

#[test]
fn a_late_pick_lands_nowhere_after_back_next_or_typing() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let picked = || Ok(Some(PathBuf::from("/picked")));
    let mut page = open(&paths, &host);

    // Browse on Other, Back, then the folder page opens before it answers.
    let _ = page.update(Event::Other, false);
    let _ = page.update(Event::BrowseOtherFile, false);
    let browsing = page.picker_sequence;
    let _ = page.update(Event::Back, false);
    let _ = page.update(Event::Install(0), false);
    let _ = page.update(Event::Picked(browsing, picked()), false);
    assert_eq!(page.folder, "", "Back");

    // Browse on the folder page, then Next.
    let _ = page.update(Event::BrowseFolder, false);
    let browsing = page.picker_sequence;
    let _ = page.update(Event::Next, false);
    let _ = page.update(Event::Picked(browsing, picked()), false);
    assert_eq!(page.folder, "", "Next");

    // Browse, then type.
    for (step, typed) in [
        (Step::Folder, Event::Folder("~/typed".into())),
        (Step::Other, Event::OtherFile("~/typed".into())),
        (Step::Custom, Event::CustomExe("~/typed".into())),
    ] {
        page.step = step;
        let browse = match step {
            Step::Folder => Event::BrowseFile,
            Step::Other => Event::BrowseOtherFile,
            _ => Event::BrowseCustomExe,
        };
        let _ = page.update(browse, false);
        let browsing = page.picker_sequence;
        let _ = page.update(typed, false);
        let _ = page.update(Event::Picked(browsing, picked()), false);
        let shown = match step {
            Step::Folder => &page.folder,
            Step::Other => &page.other_file,
            _ => &page.custom.exe,
        };
        assert_eq!(shown, "~/typed", "{step:?}");
    }
    // A pick that's still current lands.
    let _ = page.update(Event::BrowseCustomExe, false);
    let _ = page.update(Event::Picked(page.picker_sequence, picked()), false);
    assert_eq!(page.custom.exe, "/picked");
}

#[test]
fn editing_the_folder_while_it_is_looked_at_drops_the_lookup() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let cores = user_cores(temp.path());
    core(&cores, "mgba", "mGBA", "game_boy_advance", "gba");
    let folder = roms(temp.path(), &["Metroid Fusion.gba"]);
    for emulator in [Emulator::Mgba, Emulator::RetroArch] {
        let mut page = open(&paths, &host);
        let _ = page.update(Event::Install(install_row(&page, emulator)), false);
        let _ = page.update(Event::Folder(folder.display().to_string()), false);
        let _ = page.update(Event::Next, false);
        assert!(page.waiting());
        let generation = page.generation;
        let _ = page.update(Event::Folder("~/elsewhere".into()), false);
        assert!(!page.waiting(), "{emulator:?}");
        let late = match emulator {
            Emulator::RetroArch => Event::Cores(
                generation,
                work::matching_cores(
                    &paths,
                    &host,
                    page.chosen.as_ref().unwrap().retroarch().unwrap(),
                    &folder,
                ),
            ),
            _ => Event::Listed(
                generation,
                work::listing(&paths, &Scan::BuiltIn(emulator), &folder),
            ),
        };
        let _ = page.update(late, false);
        assert_eq!(page.step, Step::Folder, "{emulator:?}");
        assert!(
            page.listing.is_none() && page.cores.is_none(),
            "{emulator:?}"
        );
        assert!(!page.waiting(), "{emulator:?}");
    }
}

#[test]
fn fullscreen_and_art_hold_still_while_the_games_are_checked() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let folder = roms(temp.path(), &["Metroid Fusion.gba"]);
    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::Mgba, &folder);
    let _ = page.update(Event::SelectAll, true);
    let _ = page.update(Event::ImportSelected, true);
    assert_eq!(page.status, "Checking games...");
    let _ = page.update(Event::Fullscreen(false), true);
    let _ = page.update(Event::FetchArt(true), true);
    assert!(page.fullscreen && !page.fetch_art);
    settle(&mut page);
    let shown = texts(&page, true);
    assert!(
        shown.iter().any(|text| text == "Fullscreen: on"),
        "{shown:?}"
    );
    assert!(shown.iter().any(|text| text == "Art: none"), "{shown:?}");
    assert!(
        page.review.as_ref().unwrap().ready[0]
            .target
            .args
            .contains(&"-f".to_owned())
    );
}

#[test]
fn a_one_core_folder_keeps_its_picks_through_back_and_next() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let cores = user_cores(temp.path());
    core(&cores, "mgba", "mGBA", "game_boy_advance", "gba");
    let folder = roms(temp.path(), &["Metroid Fusion.gba", "Zelda.gba"]);
    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::RetroArch, &folder);
    assert_eq!(page.step, Step::Games);
    let _ = page.update(Event::Toggle(1), false);
    let picked = page.grid.chosen.clone();
    assert_eq!(picked.len(), 1);
    let _ = page.update(Event::Back, false);
    assert_eq!(page.step, Step::Folder);
    let _ = page.update(Event::Next, false);
    settle(&mut page);
    assert_eq!(page.step, Step::Games);
    assert_eq!(page.grid.chosen, picked);
}

#[test]
fn two_names_that_arent_utf8_are_picked_apart() {
    use std::os::unix::ffi::OsStrExt;
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let folder = roms(temp.path(), &[]);
    for name in [&b"Game \xfe.gba"[..], &b"Game \xff.gba"[..]] {
        fs::write(folder.join(std::ffi::OsStr::from_bytes(name)), "").unwrap();
    }
    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::Mgba, &folder);
    // A scan calls them unusable today; the picks mustn't rely on that.
    for entry in &mut page.listing.as_mut().unwrap().entries {
        assert!(matches!(entry.state, EntryState::Unusable(_)));
        entry.state = EntryState::New;
    }
    assert_eq!(page.listing.as_ref().unwrap().entries.len(), 2);
    let _ = page.update(Event::Toggle(0), false);
    assert_eq!(page.grid.chosen.len(), 1);
    let (_, entries, _) = page.plan_inputs().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].rom,
        page.listing.as_ref().unwrap().entries[0].rom
    );
}

#[test]
fn back_from_other_or_custom_before_next_lands_on_its_row() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    for (open_page, row) in [(Event::Other, OTHER_ROW), (Event::Custom, CUSTOM_ROW)] {
        let mut page = open(&paths, &host);
        let _ = page.update(open_page, false);
        let _ = page.update(Event::Back, false);
        assert_eq!(page.step, Step::Emulator);
        assert_eq!(page.rows().unwrap().focus.as_deref(), Some(row));
    }
}

#[test]
fn the_custom_page_refuses_a_windows_program_as_the_import_does() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let mut page = open(&paths, &host);
    let _ = page.update(Event::Custom, false);
    for exe in ["~/Games/setup.exe", r"C:\Games\eden.EXE", "eden.bat"] {
        let _ = page.update(Event::CustomExe(exe.into()), false);
        let _ = page.update(Event::CustomArgs("{rom}".into()), false);
        let _ = page.update(Event::CustomExtensions("nsp".into()), false);
        let _ = page.update(Event::Next, false);
        assert_eq!(page.step, Step::Custom);
        assert_eq!(
            page.error,
            Some(format!("`{exe}` is a Windows program, not a native one.")),
        );
    }
}

#[test]
fn a_home_path_or_a_single_game_file_lists_its_games() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let folder = roms(
        &temp.path().join("home"),
        &["Metroid Fusion.gba", "Zelda.gba"],
    );
    let mut page = open(&paths, &host);
    let _ = page.update(Event::Install(install_row(&page, Emulator::Mgba)), false);
    let _ = page.update(Event::Folder("~/roms".into()), false);
    let _ = page.update(Event::Next, false);
    settle(&mut page);
    assert_eq!(page.step, Step::Games, "{:?}", page.error);
    assert_eq!(page.listed_folder.as_deref(), Some(folder.as_path()));
    assert_eq!(titles(&page), ["Metroid Fusion", "Zelda"]);

    let _ = page.update(Event::Back, false);
    let _ = page.update(Event::Folder("~/roms/Zelda.gba".into()), false);
    let _ = page.update(Event::Next, false);
    settle(&mut page);
    assert_eq!(page.step, Step::Games, "{:?}", page.error);
    assert_eq!(titles(&page), ["Zelda"]);
}

#[test]
fn a_late_listing_or_plan_after_back_lands_nowhere() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let folder = roms(temp.path(), &["Metroid Fusion.gba"]);
    let mut page = open(&paths, &host);
    let _ = page.update(Event::Install(install_row(&page, Emulator::Mgba)), false);
    let _ = page.update(Event::Folder(folder.display().to_string()), false);
    let _ = page.update(Event::Next, false);
    let generation = page.generation;
    let listing = work::listing(&paths, &Scan::BuiltIn(Emulator::Mgba), &folder);
    let _ = page.update(Event::Back, false);
    assert_eq!(page.step, Step::Emulator);
    let _ = page.update(Event::Listed(generation, listing), false);
    assert_eq!(page.step, Step::Emulator);
    assert!(page.listing.is_none());

    to_folder(&mut page, Emulator::Mgba, &folder);
    let _ = page.update(Event::SelectAll, false);
    let _ = page.update(Event::ImportSelected, false);
    let generation = page.generation;
    let (emulating, entries, system) = page.plan_inputs().unwrap();
    let planned = work::plan(&paths, &emulating, entries, system.as_deref());
    let _ = page.update(Event::Back, false);
    assert_eq!(page.step, Step::Folder);
    let _ = page.update(Event::Planned(generation, planned), false);
    assert_eq!(page.step, Step::Folder);
    assert!(page.review.is_none());
}

#[test]
fn cancel_while_importing_finishes_with_what_was_done() {
    let temp = TempDir::new().unwrap();
    let paths = paths(temp.path(), false);
    let host = host(temp.path());
    let folder = roms(temp.path(), &["Metroid Fusion.gba", "Zelda.gba"]);
    let mut page = open(&paths, &host);
    to_folder(&mut page, Emulator::Mgba, &folder);
    let _ = page.update(Event::SelectAll, false);
    let _ = page.update(Event::ImportSelected, false);
    settle(&mut page);
    let task = page.update(Event::Import, false);
    assert_eq!(page.step, Step::Working);
    let _ = page.update(Event::Cancel, false);
    assert_eq!(page.status, "Canceling...");
    assert!(page.token.as_ref().unwrap().is_canceled());
    for event in drive(None, task) {
        let _ = page.update(event, false);
    }
    assert_eq!(page.step, Step::Finished);
    assert!(!page.busy() && !page.waiting());
    let report = page.report.as_ref().unwrap();
    assert!(report.canceled, "{report:?}");
    assert!(report.done.is_empty(), "{report:?}");
    let shown = texts(&page, false);
    assert!(
        shown
            .iter()
            .any(|text| text == "Completed imports remain after canceling."),
        "{shown:?}"
    );
}

