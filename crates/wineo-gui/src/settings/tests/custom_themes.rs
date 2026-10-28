use std::sync::{Arc, Mutex};

use iced::advanced::widget::Operation;
use iced::keyboard::{Key, Modifiers, key::Named};
use iced::theme::Mode;
use iced::widget::Id;
use iced::{Color, Rectangle};

use super::*;
use crate::layout_probe::lay_out;
use crate::theme::{PRESETS, Palette, Preset};
use appearance::{Edit, NAME_TAKEN, READABILITY_ID, Slot};
use theme_colors::{
    COPY_ID, DELETE_CONFIRM_ID, DELETE_ID, INVALID_COLOR, KEEP_ID, NAME_ID, RENAME_APPLY_ID,
    RENAME_CANCEL_ID, RENAME_ID, color_id,
};

const SIZE: iced::Size = iced::Size::new(900.0, 3000.0);

fn page_of(app: &App) -> &Page {
    app.settings.as_ref().unwrap()
}

fn form(app: &App) -> &appearance::Form {
    page_of(app).appearance().unwrap()
}

fn palette(id: &str) -> Palette {
    Preset::find(id).unwrap().palette
}

fn edit(app: &mut App, edit: Edit) {
    let _ = app.update(Message::Settings(Event::Appearance(edit)));
}

fn pick(app: &mut App, slot: Slot, id: &str) {
    edit(app, Edit::Theme(slot, id.to_owned()));
}

fn show_appearance(app: &mut App) {
    let _ = app.update(Message::Settings(Event::Tab(Tab::Appearance)));
}

fn focusable_ids(app: &App, paths: &Paths) -> Vec<Option<String>> {
    let mut laid = lay_out(page_of(app).view(paths, Palette::DARK), SIZE);
    crate::focus_button::focusables(&mut laid)
}

fn id(name: &str) -> Option<String> {
    Some(format!("{:?}", Id::from(name.to_owned())))
}

/// Every piece of text the page shows.
fn texts(app: &App, paths: &Paths) -> Vec<String> {
    struct All(Arc<Mutex<Vec<String>>>);
    impl Operation for All {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn text(&mut self, _id: Option<&Id>, _bounds: Rectangle, text: &str) {
            self.0.lock().unwrap().push(text.to_owned());
        }
    }
    let found = Arc::new(Mutex::new(Vec::new()));
    let mut laid = lay_out(page_of(app).view(paths, Palette::DARK), SIZE);
    laid.run(Box::new(All(found.clone())));
    found.lock().unwrap().clone()
}

fn write_library(paths: &Paths, text: &str) {
    let path = paths.library_file();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn library_text(paths: &Paths) -> String {
    fs::read_to_string(paths.library_file()).unwrap_or_default()
}

fn theme_path(paths: &Paths, id: &str) -> std::path::PathBuf {
    paths.themes_dir().join(format!("{id}.toml"))
}

fn write_theme(paths: &Paths, id: &str, text: &str) {
    fs::create_dir_all(paths.themes_dir()).unwrap();
    fs::write(theme_path(paths, id), text).unwrap();
}

fn theme_text(paths: &Paths, id: &str) -> String {
    fs::read_to_string(theme_path(paths, id)).unwrap_or_default()
}

const NIGHT: &str = "name = \"Night\"\nkind = \"dark\"\nbased_on = \"dracula\"\n\n[colors]\nprimary = \"#123456\"\n";
const DAY: &str = "name = \"Day\"\nkind = \"light\"\nbased_on = \"catppuccin-latte\"\n";

fn night_palette() -> Palette {
    Palette {
        primary: Color::from_rgb8(0x12, 0x34, 0x56),
        ..palette("dracula")
    }
}

/// Runs a save the way the worker does and lands it and the reload after.
fn save(app: &mut App, paths: &Paths) -> Report {
    let _ = app.update(Message::Settings(Event::Save));
    let page = page_of(app);
    assert!(page.saving, "Save started");
    let View::Global(editor) = &page.view else {
        panic!("global form expected");
    };
    let report = load::save_global_page(
        paths,
        &editor.form.edits(),
        editor.appearance.edits().as_ref(),
    )
    .unwrap();
    let generation = page.generation;
    let _ = app.update(Message::Settings(Event::Saved(
        generation,
        Ok(report.clone()),
    )));
    let _ = app.update(Message::Settings(Event::Loaded(
        generation,
        load::snapshot_global(paths),
    )));
    report
}

fn check_global(app: &mut App, paths: &Paths) {
    let page = app.settings.as_mut().unwrap();
    let sequence = start_check(page);
    let generation = page.generation;
    let _ = app.update(Message::Settings(Event::Checked(
        generation,
        sequence,
        Texts::read_global(paths),
    )));
    if page_of(app).loading() {
        let _ = app.update(Message::Settings(Event::Loaded(
            generation,
            load::snapshot_global(paths),
        )));
    }
}

/// The id of the custom theme called `name` in the draft.
fn draft_id(app: &App, name: &str) -> String {
    form(app)
        .themes
        .iter()
        .find(|theme| theme.name == name)
        .unwrap_or_else(|| panic!("no {name} in {:?}", form(app).themes))
        .id
        .clone()
}

fn names(app: &App) -> Vec<String> {
    let mut names: Vec<_> = form(app)
        .themes
        .iter()
        .map(|theme| theme.name.clone())
        .collect();
    names.sort();
    names
}

#[test]
fn custom_themes_join_the_lists_after_the_built_ins_and_paint_at_startup() {
    let (_root, paths) = setup();
    write_theme(&paths, "night", NIGHT);
    write_theme(&paths, "day", DAY);
    write_theme(&paths, "broken", "name = [\n");
    write_library(
        &paths,
        "[appearance]\ndark = \"custom:night\"\nlight = \"custom:day\"\n",
    );
    let mut app = App::boot(paths.clone()).0;
    assert_eq!(app.palette, night_palette());
    let _ = app.update(Message::ThemeChanged(Mode::Light));
    assert_eq!(app.palette, palette("catppuccin-latte"));
    let _ = app.update(Message::ThemeChanged(Mode::Dark));

    let _ = app.open_global_settings();
    let generation = page_of(&app).generation;
    let _ = app.update(Message::Settings(Event::Loaded(
        generation,
        load::snapshot_global(&paths),
    )));
    show_appearance(&mut app);
    let ids = focusable_ids(&app, &paths);
    let row = |slot: Slot, theme: &str| id(&slot.row_id(theme));
    let position = |wanted| ids.iter().position(|found| *found == wanted).unwrap();
    assert!(
        position(row(Slot::Dark, "custom:night"))
            > position(row(Slot::Dark, "catppuccin-macchiato"))
    );
    assert!(
        position(row(Slot::Light, "custom:day")) > position(row(Slot::Light, "catppuccin-latte"))
    );
    assert!(
        !ids.contains(&row(Slot::Dark, "custom:day")),
        "kinds stay apart"
    );
    assert!(!ids.iter().flatten().any(|found| found.contains("broken")));
    let choices = appearance::choices(&form(&app).themes);
    let custom: Vec<_> = choices[PRESETS.len()..]
        .iter()
        .map(|choice| (choice.id.as_str(), choice.description.as_str()))
        .collect();
    assert_eq!(
        custom,
        [
            ("custom:day", "Custom, based on Catppuccin Latte."),
            ("custom:night", "Custom, based on Dracula."),
        ]
    );
}

#[test]
fn a_custom_id_whose_file_is_gone_or_broken_falls_back() {
    let (_root, paths) = setup();
    write_theme(&paths, "broken", "name = [\n");
    write_library(
        &paths,
        "[appearance]\ndark = \"custom:broken\"\nlight = \"custom:gone\"\n",
    );
    let mut app = App::boot(paths.clone()).0;
    assert_eq!(app.palette, palette("midnight"));
    let _ = app.update(Message::ThemeChanged(Mode::Light));
    assert_eq!(app.palette, palette("daylight"));
}

#[test]
fn copies_are_named_after_their_source_and_avoid_taken_names() {
    let (_root, paths) = setup();
    write_theme(&paths, "taken", "name = \"MIDNIGHT COPY\"\n");
    let mut app = open_global_app(&paths);
    edit(&mut app, Edit::Copy(Slot::Dark));
    assert_eq!(names(&app), ["MIDNIGHT COPY", "Midnight copy 2"]);
    let first = draft_id(&app, "Midnight copy 2");
    assert_eq!(form(&app).draft.dark, first);
    // A copy of the copy is named after it.
    edit(&mut app, Edit::Copy(Slot::Dark));
    assert!(names(&app).contains(&"Midnight copy 2 copy".to_owned()));
    // A pending copy's name is taken as well.
    pick(&mut app, Slot::Dark, "midnight");
    edit(&mut app, Edit::Copy(Slot::Dark));
    let second = draft_id(&app, "Midnight copy 3");
    assert_ne!(first, second);
    assert_eq!(form(&app).draft.dark, second);
    let copy = &form(&app).themes.iter().find(|t| t.id == second).unwrap();
    assert_eq!(
        copy.based_on.as_deref(),
        Some("midnight"),
        "the source's id"
    );
    assert_eq!(copy.kind, crate::theme::PresetKind::Dark);
    assert_eq!(copy.colors, crate::theme::colors_of(palette("midnight")));
    assert_eq!(page_of(&app).changes(), 4, "three themes and the slot");
}

#[test]
fn a_copy_of_a_custom_theme_keeps_its_based_on() {
    let (_root, paths) = setup();
    write_theme(&paths, "night", NIGHT);
    write_library(&paths, "[appearance]\ndark = \"custom:night\"\n");
    let mut app = open_global_app(&paths);
    edit(&mut app, Edit::Copy(Slot::Dark));
    let copy = draft_id(&app, "Night copy");
    let theme = form(&app).themes.iter().find(|t| t.id == copy).unwrap();
    assert_eq!(theme.based_on.as_deref(), Some("dracula"));
    assert_eq!(theme.colors, crate::theme::colors_of(night_palette()));
    assert_eq!(app.palette, night_palette());
}

#[test]
fn a_copy_takes_the_place_of_its_source_in_the_slot_showing_and_previews() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    pick(&mut app, Slot::Light, "cellar-light");
    let _ = app.update(Message::ThemeChanged(Mode::Light));
    show_appearance(&mut app);
    // The Copy control copies the theme showing, here the light one.
    let mut laid = lay_out(page_of(&app).view(&paths, Palette::DARK), SIZE);
    laid.run(Box::new(
        iced::advanced::widget::operation::focusable::focus::<()>(Id::new(COPY_ID)),
    ));
    let (sent, _) = crate::focus_button::press(&mut laid, Named::Enter, Modifiers::empty());
    assert_eq!(
        sent.iter().map(|m| format!("{m:?}")).collect::<Vec<_>>(),
        [format!(
            "{:?}",
            Message::Settings(Event::Appearance(Edit::Copy(Slot::Light)))
        )]
    );
    drop(laid);
    edit(&mut app, Edit::Copy(Slot::Light));
    let copy = draft_id(&app, "Cellar Light copy");
    assert_eq!(form(&app).draft.light, copy);
    assert_eq!(form(&app).draft.dark, "midnight");
    assert_eq!(app.palette, palette("cellar-light"));
    // Now the copy is the one showing, and its fields edit the preview.
    edit(&mut app, Edit::Color(copy.clone(), 5, "#102030".into()));
    assert_eq!(app.palette.primary, Color::from_rgb8(0x10, 0x20, 0x30));

    // Not following, the single slot takes the copy.
    edit(&mut app, Edit::FollowSystem(false));
    edit(&mut app, Edit::Copy(Slot::Theme));
    let second = draft_id(&app, "Cellar Light copy copy");
    assert_eq!(form(&app).draft.theme, second);
    assert_eq!(form(&app).draft.light, copy);
}

#[test]
fn a_bad_hex_is_refused_with_the_message_and_the_swatch_keeps_the_last_good_color() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::Copy(Slot::Dark));
    let copy = draft_id(&app, "Midnight copy");
    edit(&mut app, Edit::Color(copy.clone(), 5, "#ABCDEF".into()));
    let good = Color::from_rgb8(0xab, 0xcd, 0xef);
    assert_eq!(app.palette.primary, good);
    assert!(!texts(&app, &paths).contains(&INVALID_COLOR.to_owned()));
    for bad in ["#abcde", "abcdef", "#ggggggg", ""] {
        edit(&mut app, Edit::Color(copy.clone(), 5, bad.into()));
        assert_eq!(app.palette.primary, good, "{bad:?}");
        assert!(form(&app).color_invalid(&copy, 5));
        let theme = form(&app).themes.iter().find(|t| t.id == copy).unwrap();
        assert_eq!(theme.colors.primary.as_deref(), Some("#abcdef"));
        assert_eq!(form(&app).color_text(theme, 5), bad, "the field keeps it");
        let shown = texts(&app, &paths);
        assert_eq!(
            shown.iter().filter(|text| *text == INVALID_COLOR).count(),
            1,
            "{bad:?} {shown:?}"
        );
    }
    edit(&mut app, Edit::Color(copy.clone(), 5, "#010203".into()));
    assert!(!form(&app).color_invalid(&copy, 5));
    assert!(!texts(&app, &paths).contains(&INVALID_COLOR.to_owned()));
}

#[test]
fn rename_refuses_a_name_another_custom_theme_has() {
    let (_root, paths) = setup();
    write_theme(&paths, "night", NIGHT);
    write_theme(&paths, "day", DAY);
    write_library(&paths, "[appearance]\ndark = \"custom:night\"\n");
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::RenameOpen("custom:night".into()));
    edit(&mut app, Edit::RenameText(" dAY ".into()));
    edit(&mut app, Edit::RenameApply);
    assert!(form(&app).renaming.as_ref().unwrap().taken);
    assert!(texts(&app, &paths).contains(&NAME_TAKEN.to_owned()));
    assert_eq!(names(&app), ["Day", "Night"]);
    assert!(!page_of(&app).is_dirty());
    // Typing again clears it; its own name in another case is fine.
    edit(&mut app, Edit::RenameText("NIGHT".into()));
    assert!(!texts(&app, &paths).contains(&NAME_TAKEN.to_owned()));
    edit(&mut app, Edit::RenameApply);
    assert!(form(&app).renaming.is_none());
    assert_eq!(names(&app), ["Day", "NIGHT"]);
    assert_eq!(page_of(&app).changes(), 1);
    // Cancel keeps the name.
    edit(&mut app, Edit::RenameOpen("custom:night".into()));
    edit(&mut app, Edit::RenameText("Other".into()));
    edit(&mut app, Edit::RenameCancel);
    assert_eq!(names(&app), ["Day", "NIGHT"]);
    // A pending copy's name is taken too.
    edit(&mut app, Edit::Copy(Slot::Dark));
    edit(&mut app, Edit::RenameOpen("custom:night".into()));
    edit(&mut app, Edit::RenameText("night COPY".into()));
    edit(&mut app, Edit::RenameApply);
    assert!(form(&app).renaming.as_ref().unwrap().taken);
}

#[test]
fn delete_asks_first_and_an_in_use_theme_falls_back_in_every_slot() {
    let (_root, paths) = setup();
    write_theme(&paths, "night", NIGHT);
    write_theme(&paths, "day", DAY);
    write_library(
        &paths,
        "[appearance]\ndark = \"custom:night\"\nlight = \"custom:day\"\ntheme = \"custom:night\"\n",
    );
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::Delete("custom:night".into()));
    assert_eq!(names(&app), ["Day", "Night"], "not without asking");
    edit(&mut app, Edit::AskDelete("custom:night".into()));
    assert!(texts(&app, &paths).contains(&"Delete Night?".to_owned()));
    edit(&mut app, Edit::KeepTheme);
    edit(&mut app, Edit::Delete("custom:night".into()));
    assert_eq!(names(&app), ["Day", "Night"], "Keep it backs out");
    edit(&mut app, Edit::AskDelete("custom:night".into()));
    edit(&mut app, Edit::Delete("custom:night".into()));
    assert_eq!(names(&app), ["Day"]);
    let draft = &form(&app).draft;
    assert_eq!(
        (draft.dark.as_str(), draft.theme.as_str()),
        ("midnight", "midnight")
    );
    assert_eq!(draft.light, "custom:day");
    assert_eq!(app.palette, palette("midnight"));
    let _ = app.update(Message::ThemeChanged(Mode::Light));
    edit(&mut app, Edit::AskDelete("custom:day".into()));
    edit(&mut app, Edit::Delete("custom:day".into()));
    assert_eq!(form(&app).draft.light, "daylight");
    assert_eq!(app.palette, palette("daylight"));
    assert_eq!(page_of(&app).changes(), 5, "three slots and two deletes");
    // A light theme in the single slot falls back to the light default.
    let (_root, paths) = setup();
    write_theme(&paths, "day", DAY);
    write_library(
        &paths,
        "[appearance]\nfollow_system = false\ntheme = \"custom:day\"\n",
    );
    let mut app = open_global_app(&paths);
    edit(&mut app, Edit::AskDelete("custom:day".into()));
    edit(&mut app, Edit::Delete("custom:day".into()));
    assert_eq!(form(&app).draft.theme, "daylight");
}

#[test]
fn save_creates_edits_renames_then_writes_library_toml_then_deletes() {
    let (_root, paths) = setup();
    write_theme(&paths, "night", NIGHT);
    write_theme(&paths, "day", DAY);
    write_theme(&paths, "old", "name = \"Old\"\n");
    write_library(&paths, "[appearance]\ndark = \"custom:old\"\n");
    let mut app = open_global_app(&paths);
    // Copy Cellar into the dark slot and change a color.
    pick(&mut app, Slot::Dark, "cellar");
    edit(&mut app, Edit::Copy(Slot::Dark));
    let copy = draft_id(&app, "Cellar copy");
    assert!(appearance::is_new(&copy));
    edit(&mut app, Edit::Color(copy.clone(), 0, "#000001".into()));
    edit(
        &mut app,
        Edit::Color("custom:night".into(), 3, "#fefefe".into()),
    );
    edit(&mut app, Edit::RenameOpen("custom:day".into()));
    edit(&mut app, Edit::RenameText("Morning".into()));
    edit(&mut app, Edit::RenameApply);
    edit(&mut app, Edit::AskDelete("custom:old".into()));
    edit(&mut app, Edit::Delete("custom:old".into()));
    let expected = Palette {
        background: Color::from_rgb8(0, 0, 1),
        ..palette("cellar")
    };
    assert_eq!(app.palette, expected);
    let report = save(&mut app, &paths);
    assert_eq!(report.failed, None);
    assert_eq!(report.created, [(copy, "custom:cellar-copy".to_owned())]);
    let created = theme_text(&paths, "cellar-copy");
    assert!(created.contains("name = \"Cellar copy\""), "{created}");
    assert!(created.contains("based_on = \"cellar\""), "{created}");
    assert!(created.contains("background = \"#000001\""), "{created}");
    assert!(theme_text(&paths, "night").contains("foreground = \"#fefefe\""));
    assert!(theme_text(&paths, "day").contains("name = \"Morning\""));
    assert!(!theme_path(&paths, "old").exists());
    assert!(library_text(&paths).contains("dark = \"custom:cellar-copy\""));
    assert!(!page_of(&app).is_dirty(), "{:?}", form(&app));
    assert_eq!(form(&app).draft.dark, "custom:cellar-copy");
    assert_eq!(app.palette, expected);
    // Leaving keeps the saved custom theme painted.
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    assert!(app.settings.is_none());
    assert_eq!(app.palette, expected);
}

#[test]
fn a_theme_file_that_fails_stops_the_save_before_library_toml() {
    let (_root, paths) = setup();
    write_theme(&paths, "old", "name = \"Old\"\n");
    write_theme(&paths, "new", "name = \"New\"\n");
    write_library(&paths, "[appearance]\ndark = \"custom:old\"\n");
    let before = library_text(&paths);
    let mut app = open_global_app(&paths);
    let _ = app.update(Message::Settings(Event::GlobalSwitch(
        Switch::Gamemode,
        true,
    )));
    edit(&mut app, Edit::AskDelete("custom:old".into()));
    edit(&mut app, Edit::Delete("custom:old".into()));
    edit(
        &mut app,
        Edit::Color("custom:new".into(), 0, "#000002".into()),
    );
    // A newer wineo takes the file over, so core refuses the update.
    write_theme(&paths, "new", "schema_version = 99\nname = \"New\"\n");
    let report = save(&mut app, &paths);
    assert!(report.failed.is_some());
    assert!(
        fs::read_to_string(paths.global_config())
            .unwrap()
            .contains("gamemode = true"),
        "config.toml landed first"
    );
    assert_eq!(library_text(&paths), before, "library.toml untouched");
    assert!(theme_path(&paths, "old").exists(), "no delete ran");
    let page = page_of(&app);
    assert!(page.error.is_some());
    assert_eq!(page.changes(), 3, "the slot, the delete, and the color");
    assert_eq!(form(&app).draft.dark, "midnight");
    assert!(!names(&app).contains(&"Old".to_owned()), "the delete stays");
    assert_eq!(app.palette, palette("midnight"));
}

#[test]
fn a_create_that_fails_keeps_every_appearance_edit() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    edit(&mut app, Edit::Copy(Slot::Dark));
    let copy = draft_id(&app, "Midnight copy");
    edit(&mut app, Edit::Color(copy.clone(), 5, "#102030".into()));
    // A file where the folder goes refuses every theme write.
    fs::create_dir_all(paths.themes_dir().parent().unwrap()).unwrap();
    fs::write(paths.themes_dir(), "").unwrap();
    let report = save(&mut app, &paths);
    assert!(report.failed.is_some());
    assert!(report.created.is_empty());
    assert!(!library_text(&paths).contains("custom:"));
    assert_eq!(form(&app).draft.dark, copy, "still the pending copy");
    assert_eq!(page_of(&app).changes(), 2);
    assert_eq!(app.palette.primary, Color::from_rgb8(0x10, 0x20, 0x30));
}

#[test]
fn a_refused_library_toml_keeps_the_created_theme_and_maps_its_id() {
    let (_root, paths) = setup();
    write_theme(&paths, "old", "name = \"Old\"\n");
    write_library(&paths, "[appearance]\ndark = \"custom:old\"\n");
    let mut app = open_global_app(&paths);
    edit(&mut app, Edit::AskDelete("custom:old".into()));
    edit(&mut app, Edit::Delete("custom:old".into()));
    edit(&mut app, Edit::Copy(Slot::Dark));
    let copy = draft_id(&app, "Midnight copy");
    let broken = "schema_version = 99\n[appearance]\ndark = \"custom:old\"\n";
    write_library(&paths, broken);
    let report = save(&mut app, &paths);
    assert!(report.failed.is_some());
    assert_eq!(report.created, [(copy, "custom:midnight-copy".to_owned())]);
    assert!(theme_path(&paths, "midnight-copy").exists());
    assert_eq!(library_text(&paths), broken);
    assert!(
        theme_path(&paths, "old").exists(),
        "library.toml still names it, so it stays"
    );
    // The copy landed, so it's no longer an edit, but the slot and the
    // delete are, now with the real id.
    assert_eq!(form(&app).draft.dark, "custom:midnight-copy");
    assert!(
        form(&app)
            .loaded_themes
            .iter()
            .any(|theme| theme.id == "custom:midnight-copy")
    );
    assert_eq!(page_of(&app).changes(), 2, "{:?}", form(&app));
    let save = form(&app).edits().unwrap();
    assert!(save.creates.is_empty());
    assert_eq!(save.deletes, ["old"]);
    assert_eq!(app.palette, palette("midnight"));
    // Once library.toml takes writes, the rest lands without a second copy.
    write_library(&paths, "[appearance]\ndark = \"custom:old\"\n");
    let View::Global(editor) = &page_of(&app).view else {
        panic!("global form expected");
    };
    let report = load::save_global_page(
        &paths,
        &editor.form.edits(),
        editor.appearance.edits().as_ref(),
    )
    .unwrap();
    assert_eq!(report.failed, None);
    assert!(!theme_path(&paths, "midnight-copy-2").exists());
    assert!(!theme_path(&paths, "old").exists());
    assert!(library_text(&paths).contains("dark = \"custom:midnight-copy\""));
}

#[cfg(unix)]
#[test]
fn a_delete_that_fails_comes_after_library_toml_and_stays_pending() {
    use std::os::unix::fs::PermissionsExt;
    let (_root, paths) = setup();
    write_theme(&paths, "old", "name = \"Old\"\n");
    write_library(&paths, "[appearance]\ndark = \"custom:old\"\n");
    let mut app = open_global_app(&paths);
    edit(&mut app, Edit::AskDelete("custom:old".into()));
    edit(&mut app, Edit::Delete("custom:old".into()));
    let dir = paths.themes_dir();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let report = save(&mut app, &paths);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    if theme_path(&paths, "old").exists() {
        assert!(report.failed.is_some());
        assert!(library_text(&paths).contains("dark = \"midnight\""));
        assert!(!form(&app).themes.iter().any(|theme| theme.name == "Old"));
        assert_eq!(page_of(&app).changes(), 1, "only the delete is left");
        assert_eq!(form(&app).edits().unwrap().deletes, ["old"]);
    } else {
        // Running as root, the folder's mode doesn't stop the delete.
        assert_eq!(report.failed, None);
    }
}

#[test]
fn revert_drops_copies_renames_deletes_and_color_edits() {
    let (_root, paths) = setup();
    write_theme(&paths, "night", NIGHT);
    write_theme(&paths, "day", DAY);
    write_library(&paths, "[appearance]\ndark = \"custom:night\"\n");
    let mut app = open_global_app(&paths);
    let saved = form(&app).clone();
    edit(
        &mut app,
        Edit::Color("custom:night".into(), 5, "#000003".into()),
    );
    edit(
        &mut app,
        Edit::Color("custom:night".into(), 4, "#zz".into()),
    );
    edit(&mut app, Edit::RenameOpen("custom:day".into()));
    edit(&mut app, Edit::RenameText("Dawn".into()));
    edit(&mut app, Edit::RenameApply);
    edit(&mut app, Edit::Copy(Slot::Dark));
    edit(&mut app, Edit::AskDelete("custom:day".into()));
    edit(&mut app, Edit::Delete("custom:day".into()));
    assert_ne!(app.palette, night_palette());
    let _ = app.update(Message::Settings(Event::Revert));
    assert!(!page_of(&app).is_dirty());
    let form = form(&app);
    assert_eq!(
        (&form.draft, &form.themes, &form.renaming, &form.deleting),
        (&saved.draft, &saved.themes, &None, &None)
    );
    assert!(!form.color_invalid("custom:night", 4));
    assert_eq!(app.palette, night_palette());
}

#[test]
fn discard_drops_pending_theme_edits() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    edit(&mut app, Edit::Copy(Slot::Dark));
    let copy = draft_id(&app, "Midnight copy");
    edit(&mut app, Edit::Color(copy, 0, "#000004".into()));
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    let _ = app.update(Message::Settings(Event::Discard));
    assert!(app.settings.is_none());
    assert_eq!(app.palette, palette("midnight"));
    assert!(!paths.themes_dir().exists());
}

#[test]
fn a_newer_theme_file_is_read_only_with_copy_offered() {
    let (_root, paths) = setup();
    write_theme(
        &paths,
        "future",
        "schema_version = 2\nname = \"Future\"\nbased_on = \"cellar\"\n",
    );
    write_library(&paths, "[appearance]\ndark = \"custom:future\"\n");
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    assert_eq!(app.palette, palette("cellar"));
    let ids = focusable_ids(&app, &paths);
    assert!(ids.contains(&id(COPY_ID)), "{ids:?}");
    for absent in [RENAME_ID, DELETE_ID, &color_id(0)] {
        assert!(!ids.contains(&id(absent)), "{absent} {ids:?}");
    }
    assert!(
        texts(&app, &paths)
            .contains(&"A newer wineo made this theme, so it's read-only.".to_owned())
    );
    edit(
        &mut app,
        Edit::Color("custom:future".into(), 0, "#000005".into()),
    );
    edit(&mut app, Edit::RenameOpen("custom:future".into()));
    edit(&mut app, Edit::AskDelete("custom:future".into()));
    assert!(!page_of(&app).is_dirty());
    assert!(form(&app).renaming.is_none() && form(&app).deleting.is_none());
    edit(&mut app, Edit::Copy(Slot::Dark));
    let copy = draft_id(&app, "Future copy");
    assert_eq!(form(&app).draft.dark, copy);
    assert!(focusable_ids(&app, &paths).contains(&id(RENAME_ID)));
}

#[test]
fn a_built_in_shows_its_roles_read_only() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    let shown = texts(&app, &paths);
    assert!(shown.contains(&"Built-in themes are read-only.".to_owned()));
    assert!(shown.contains(&"#6d8cff".to_owned()), "{shown:?}");
    let ids = focusable_ids(&app, &paths);
    assert!(!ids.contains(&id(&color_id(0))));
    assert!(!ids.contains(&id(RENAME_ID)));
}

#[test]
fn turning_following_off_starts_on_the_theme_showing() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    pick(&mut app, Slot::Light, "cellar-light");
    let _ = app.update(Message::ThemeChanged(Mode::Light));
    assert_eq!(app.palette, palette("cellar-light"));
    edit(&mut app, Edit::FollowSystem(false));
    assert_eq!(form(&app).draft.theme, "cellar-light");
    assert_eq!(app.palette, palette("cellar-light"), "nothing changes");
    assert_eq!(page_of(&app).changes(), 3);
    // Turning it back on leaves the single theme alone.
    edit(&mut app, Edit::FollowSystem(true));
    assert_eq!(form(&app).draft.theme, "cellar-light");
}

#[test]
fn swatches_show_the_adjusted_palette_with_improve_readability() {
    let dracula = Preset::find("dracula").unwrap();
    assert_eq!(
        appearance::swatch_palette(dracula.palette, dracula.kind, false),
        dracula.palette
    );
    assert_eq!(
        appearance::swatch_palette(dracula.palette, dracula.kind, true),
        crate::theme::improve(dracula.palette, dracula.kind)
    );
    // The role swatches follow the setting; the hex shows what's stored.
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    pick(&mut app, Slot::Dark, "dracula");
    edit(&mut app, Edit::ImproveReadability(true));
    show_appearance(&mut app);
    assert!(texts(&app, &paths).contains(&"#6272a4".to_owned()));
}

#[test]
fn a_change_on_disk_under_appearance_edits_names_the_file() {
    let (_root, paths) = setup();
    write_theme(&paths, "night", NIGHT);
    let mut app = open_global_app(&paths);
    pick(&mut app, Slot::Dark, "cellar");
    write_theme(&paths, "night", "name = \"Night\"\n");
    check_global(&mut app, &paths);
    let page = page_of(&app);
    assert!(page.changed_on_disk);
    assert_eq!(page.changed_file, ChangedFile::Theme);
    assert!(texts(&app, &paths).contains(&"A theme file changed on disk".to_owned()));

    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    pick(&mut app, Slot::Dark, "cellar");
    write_library(&paths, "[appearance]\ndark = \"dracula\"\n");
    check_global(&mut app, &paths);
    assert_eq!(page_of(&app).changed_file, ChangedFile::Library);
    assert!(texts(&app, &paths).contains(&"library.toml changed on disk".to_owned()));

    // config.toml is named first when it moved too.
    write_global(&paths, "gamemode = true\n");
    check_global(&mut app, &paths);
    assert_eq!(page_of(&app).changed_file, ChangedFile::Config);
    assert!(texts(&app, &paths).contains(&"The config file changed on disk".to_owned()));
}

#[test]
fn a_hand_edit_to_a_theme_file_reloads_a_clean_tab() {
    let (_root, paths) = setup();
    write_theme(&paths, "night", NIGHT);
    write_library(&paths, "[appearance]\ndark = \"custom:night\"\n");
    let mut app = open_global_app(&paths);
    let _ = app.update(Message::Settings(Event::GlobalSwitch(
        Switch::Gamemode,
        true,
    )));
    assert_eq!(app.palette, night_palette());
    write_theme(&paths, "night", "name = \"Night\"\nbased_on = \"cellar\"\n");
    check_global(&mut app, &paths);
    let page = page_of(&app);
    assert!(!page.changed_on_disk);
    assert_eq!(page.changes(), 1, "General's edit is kept");
    assert_eq!(app.palette, palette("cellar"));
    // A new file shows up in the lists too.
    write_theme(&paths, "day", DAY);
    check_global(&mut app, &paths);
    assert_eq!(names(&app), ["Day", "Night"]);
}

#[test]
fn keyboard_reaches_every_colors_control() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::Copy(Slot::Dark));
    let copy = draft_id(&app, "Midnight copy");
    let mut wanted: Vec<String> = [COPY_ID, RENAME_ID, DELETE_ID].map(str::to_owned).to_vec();
    wanted.extend((0..9).map(color_id));
    let ids = focusable_ids(&app, &paths);
    let positions: Vec<_> = wanted
        .iter()
        .map(|wanted| {
            ids.iter()
                .position(|found| *found == id(wanted))
                .unwrap_or_else(|| panic!("{wanted} {ids:?}"))
        })
        .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "{ids:?}"
    );
    assert_eq!(ids.last(), Some(&id(READABILITY_ID)));
    edit(&mut app, Edit::RenameOpen(copy.clone()));
    let ids = focusable_ids(&app, &paths);
    for wanted in [NAME_ID, RENAME_APPLY_ID, RENAME_CANCEL_ID] {
        assert!(ids.contains(&id(wanted)), "{wanted} {ids:?}");
    }
    edit(&mut app, Edit::RenameCancel);
    edit(&mut app, Edit::AskDelete(copy));
    let ids = focusable_ids(&app, &paths);
    for wanted in [KEEP_ID, DELETE_CONFIRM_ID] {
        assert!(ids.contains(&id(wanted)), "{wanted} {ids:?}");
    }
    // Tab walks onto them like the rest of the page.
    let mut keys = open_global_app(&paths);
    let mut laid = lay_out(page_of(&app).view(&paths, Palette::DARK), SIZE);
    let mut reached = Vec::new();
    for _ in 0..ids.len() {
        let task = keys.settings_key(&Key::Named(Named::Tab), Modifiers::empty(), false);
        crate::focus_button::run_task(&mut laid, task);
        reached.extend(laid.focused());
    }
    for target in [COPY_ID, KEEP_ID, DELETE_CONFIRM_ID, &color_id(8)] {
        assert!(
            reached.contains(&Id::from(target.to_owned())),
            "{target} {reached:?}"
        );
    }
}

#[test]
fn a_save_locks_the_colors_controls() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::Copy(Slot::Dark));
    let _ = app.update(Message::Settings(Event::Save));
    let ids = focusable_ids(&app, &paths);
    for absent in [COPY_ID, RENAME_ID, DELETE_ID] {
        assert!(!ids.contains(&id(absent)), "{absent} {ids:?}");
    }
    let copy = draft_id(&app, "Midnight copy");
    edit(&mut app, Edit::Color(copy.clone(), 0, "#000006".into()));
    let theme = form(&app).themes.iter().find(|t| t.id == copy).unwrap();
    assert_ne!(theme.colors.background.as_deref(), Some("#000006"));
}
