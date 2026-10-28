use iced::keyboard::{Key, Modifiers, key::Named};
use iced::theme::Mode;
use iced::widget::Id;

use super::*;
use crate::layout_probe::lay_out;
use crate::theme::{Palette, Preset};
use appearance::{Edit, FOLLOW_ID, READABILITY_ID, Slot};

const SIZE: iced::Size = iced::Size::new(900.0, 2000.0);

fn page_of(app: &App) -> &Page {
    app.settings.as_ref().unwrap()
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

fn rows(slot: Slot, ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| slot.row_id(id)).collect()
}

const DARK: [&str; 5] = [
    "midnight",
    "cellar",
    "high-contrast",
    "dracula",
    "catppuccin-macchiato",
];
const LIGHT: [&str; 3] = ["daylight", "cellar-light", "catppuccin-latte"];
const ALL: [&str; 8] = [
    "midnight",
    "daylight",
    "cellar",
    "cellar-light",
    "high-contrast",
    "dracula",
    "catppuccin-macchiato",
    "catppuccin-latte",
];

fn write_library(paths: &Paths, text: &str) {
    let path = paths.library_file();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn library_text(paths: &Paths) -> String {
    fs::read_to_string(paths.library_file()).unwrap_or_default()
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

/// A check the way the tick runs it, landing its reload if it starts one.
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

#[test]
fn following_the_system_lists_dark_then_light_themes() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    let mut expected: Vec<_> = Tab::ALL.iter().map(|tab| id(tab.id())).collect();
    expected.push(id(FOLLOW_ID));
    expected.extend(rows(Slot::Dark, &DARK).iter().map(|row| id(row)));
    expected.extend(rows(Slot::Light, &LIGHT).iter().map(|row| id(row)));
    expected.push(id(READABILITY_ID));
    assert_eq!(focusable_ids(&app, &paths), expected);
}

#[test]
fn not_following_lists_every_theme_once() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::FollowSystem(false));
    let mut expected: Vec<_> = Tab::ALL.iter().map(|tab| id(tab.id())).collect();
    expected.push(id(FOLLOW_ID));
    expected.extend(rows(Slot::Theme, &ALL).iter().map(|row| id(row)));
    expected.push(id(READABILITY_ID));
    assert_eq!(focusable_ids(&app, &paths), expected);
    assert_eq!(page_of(&app).changes(), 1);
}

#[test]
fn the_lists_offer_every_built_in_with_its_kind() {
    let choices = appearance::choices();
    assert_eq!(
        choices
            .iter()
            .map(|choice| choice.id.as_str())
            .collect::<Vec<_>>(),
        ALL
    );
    for choice in &choices {
        let preset = Preset::find(&choice.id).unwrap();
        assert_eq!(choice.kind, preset.kind);
        assert_eq!(choice.name, preset.name);
        assert_eq!(choice.description, preset.description);
        assert_eq!(choice.palette, preset.palette);
    }
}

#[test]
fn up_and_down_walk_each_list_and_enter_chooses() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    {
        let mut laid = lay_out(page_of(&app).view(&paths, Palette::DARK), SIZE);
        crate::focus_button::walk_rows(&mut laid, &rows(Slot::Dark, &DARK));
        crate::focus_button::walk_rows(&mut laid, &rows(Slot::Light, &LIGHT));
        // Enter on a focused row picks it, without the mouse.
        laid.run(Box::new(
            iced::advanced::widget::operation::focusable::focus::<()>(Id::from(
                Slot::Light.row_id("catppuccin-latte"),
            )),
        ));
        let (sent, _) = crate::focus_button::press(&mut laid, Named::Enter, Modifiers::empty());
        assert_eq!(
            sent.iter().map(|m| format!("{m:?}")).collect::<Vec<_>>(),
            [format!(
                "{:?}",
                Message::Settings(Event::Appearance(Edit::Theme(
                    Slot::Light,
                    "catppuccin-latte".into()
                )))
            )]
        );
    }
    edit(&mut app, Edit::FollowSystem(false));
    let mut laid = lay_out(page_of(&app).view(&paths, Palette::DARK), SIZE);
    crate::focus_button::walk_rows(&mut laid, &rows(Slot::Theme, &ALL));
}

#[test]
fn tab_reaches_both_check_rows() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    let mut keys = open_global_app(&paths);
    let mut laid = lay_out(page_of(&app).view(&paths, Palette::DARK), SIZE);
    let mut reached = Vec::new();
    for _ in 0..focusable_ids(&app, &paths).len() {
        let task = keys.settings_key(&Key::Named(Named::Tab), Modifiers::empty(), false);
        crate::focus_button::run_task(&mut laid, task);
        reached.extend(laid.focused());
    }
    for target in [FOLLOW_ID, READABILITY_ID] {
        assert!(reached.contains(&Id::new(target)), "{target} {reached:?}");
    }
}

#[test]
fn picking_a_theme_counts_as_an_edit_and_repaints_at_once() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    assert_eq!(app.palette, palette("midnight"));
    pick(&mut app, Slot::Dark, "dracula");
    assert_eq!(app.palette, palette("dracula"));
    assert_eq!(page_of(&app).changes(), 1);
    assert!(page_of(&app).can_save());
    // Picking the saved one again is no edit.
    pick(&mut app, Slot::Dark, "midnight");
    assert_eq!(page_of(&app).changes(), 0);
    assert_eq!(app.palette, palette("midnight"));
}

#[test]
fn the_light_slot_shows_only_once_the_system_is_light() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    pick(&mut app, Slot::Light, "catppuccin-latte");
    assert_eq!(app.palette, palette("midnight"), "the system is dark");
    let _ = app.update(Message::ThemeChanged(Mode::Light));
    assert_eq!(app.palette, palette("catppuccin-latte"));
}

#[test]
fn a_system_change_mid_preview_uses_the_previewed_lists() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    pick(&mut app, Slot::Dark, "cellar");
    pick(&mut app, Slot::Light, "cellar-light");
    assert_eq!(app.palette, palette("cellar"));
    let _ = app.update(Message::ThemeChanged(Mode::Light));
    assert_eq!(app.palette, palette("cellar-light"));
    let _ = app.update(Message::ThemeChanged(Mode::Dark));
    assert_eq!(app.palette, palette("cellar"));
    // Not following, the mode changes nothing.
    edit(&mut app, Edit::FollowSystem(false));
    pick(&mut app, Slot::Theme, "high-contrast");
    let _ = app.update(Message::ThemeChanged(Mode::Light));
    assert_eq!(app.palette, palette("high-contrast"));
    let _ = app.update(Message::Settings(Event::Revert));
    assert_eq!(app.palette, palette("daylight"), "back to the saved lists");
}

#[test]
fn improve_readability_previews_the_transformed_palette() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    pick(&mut app, Slot::Dark, "dracula");
    edit(&mut app, Edit::ImproveReadability(true));
    assert_eq!(app.palette.muted, iced::Color::from_rgb8(0x7f, 0x90, 0xc4));
    assert_eq!(page_of(&app).changes(), 2);
    edit(&mut app, Edit::ImproveReadability(false));
    assert_eq!(app.palette, palette("dracula"));
    assert_eq!(page_of(&app).changes(), 1);
}

#[test]
fn save_writes_only_the_changed_appearance_fields_and_keeps_the_look() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    pick(&mut app, Slot::Dark, "dracula");
    let report = save(&mut app, &paths);
    assert_eq!(report.failed, None);
    let text = library_text(&paths);
    assert!(text.contains("[appearance]"), "{text}");
    assert!(text.contains("dark = \"dracula\""), "{text}");
    for untouched in ["follow_system", "light =", "theme =", "improve_readability"] {
        assert!(!text.contains(untouched), "{untouched} in {text}");
    }
    assert!(!page_of(&app).is_dirty());
    assert!(!paths.global_config().exists(), "General had no edits");
    assert_eq!(app.palette, palette("dracula"));
    // Leaving keeps the saved look.
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    assert!(app.settings.is_none());
    assert_eq!(app.palette, palette("dracula"));
    let _ = app.update(Message::ThemeChanged(Mode::Light));
    assert_eq!(app.palette, palette("daylight"));
}

#[test]
fn one_save_writes_both_tabs_files() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    let _ = app.update(Message::Settings(Event::GlobalSwitch(
        Switch::Gamemode,
        true,
    )));
    pick(&mut app, Slot::Light, "catppuccin-latte");
    assert_eq!(page_of(&app).changes(), 2);
    let report = save(&mut app, &paths);
    assert_eq!(report.failed, None);
    let config = fs::read_to_string(paths.global_config()).unwrap();
    assert!(config.contains("gamemode = true"), "{config}");
    assert!(library_text(&paths).contains("light = \"catppuccin-latte\""));
    assert!(!page_of(&app).is_dirty());
}

#[test]
fn revert_restores_the_saved_palette() {
    let (_root, paths) = setup();
    write_library(&paths, "[appearance]\ndark = \"cellar\"\n");
    let mut app = open_global_app(&paths);
    assert_eq!(app.palette, palette("cellar"));
    pick(&mut app, Slot::Dark, "high-contrast");
    edit(&mut app, Edit::ImproveReadability(true));
    assert_eq!(app.palette, palette("high-contrast"));
    let _ = app.update(Message::Settings(Event::Revert));
    assert!(!page_of(&app).is_dirty());
    assert_eq!(app.palette, palette("cellar"));
}

#[test]
fn discard_on_leaving_restores_the_saved_palette() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    pick(&mut app, Slot::Dark, "dracula");
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    assert!(page_of(&app).leave_prompt);
    assert_eq!(app.palette, palette("dracula"), "still previewing");
    let _ = app.update(Message::Settings(Event::Discard));
    assert!(app.settings.is_none());
    assert_eq!(app.palette, palette("midnight"));
    assert!(!library_text(&paths).contains("dracula"));
}

/// Saves a General edit and an Appearance edit against a library.toml of
/// `text`, which refuses the write, and checks what landed and what stayed.
fn refused_save(text: &str) -> (tempfile::TempDir, Paths, App) {
    let (root, paths) = setup();
    write_library(&paths, text);
    let mut app = open_global_app(&paths);
    let _ = app.update(Message::Settings(Event::GlobalSwitch(
        Switch::Gamemode,
        true,
    )));
    pick(&mut app, Slot::Dark, "dracula");
    let report = save(&mut app, &paths);
    assert!(report.failed.is_some());
    let config = fs::read_to_string(paths.global_config()).unwrap();
    assert!(
        config.contains("gamemode = true"),
        "General landed: {config}"
    );
    assert_eq!(library_text(&paths), text);
    let page = page_of(&app);
    assert_eq!(page.changes(), 1, "only the Appearance edit is left");
    assert_eq!(
        global_editor(page).form.draft.gamemode,
        global_editor(page).form.loaded.gamemode
    );
    assert_eq!(
        page.appearance().unwrap().draft.dark,
        "dracula",
        "the edit stays"
    );
    assert!(page.error.is_some());
    assert_eq!(app.palette, palette("dracula"), "the preview stays");
    (root, paths, app)
}

#[test]
fn a_library_toml_checkup_can_repair_offers_open_checkup() {
    for text in ["schema_version = [\n", "schema_version = 0\n"] {
        let (_root, paths, mut app) = refused_save(text);
        assert!(page_of(&app).error_checkup, "{text:?}");
        let ids = focusable_ids(&app, &paths);
        assert!(ids.contains(&id(view::CHECKUP_ID)), "{text:?} {ids:?}");
        // Any edit clears the report and its action.
        pick(&mut app, Slot::Dark, "cellar");
        assert!(!page_of(&app).error_checkup);
        assert!(!focusable_ids(&app, &paths).contains(&id(view::CHECKUP_ID)));
    }
}

#[test]
fn a_library_toml_from_a_newer_wineo_says_to_update_without_open_checkup() {
    let (_root, paths, app) = refused_save("schema_version = 99\n");
    let page = page_of(&app);
    assert!(!page.error_checkup);
    assert!(
        page.error.as_deref().unwrap().contains("update wineo"),
        "{:?}",
        page.error
    );
    assert!(!focusable_ids(&app, &paths).contains(&id(view::CHECKUP_ID)));
}

#[test]
fn a_general_failure_stops_before_library_toml() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    let _ = app.update(Message::Settings(Event::GlobalSwitch(
        Switch::Gamemode,
        true,
    )));
    pick(&mut app, Slot::Dark, "dracula");
    // A newer config refuses the write.
    write_global(&paths, "schema_version = 99\n");
    let View::Global(editor) = &page_of(&app).view else {
        panic!("global form expected");
    };
    let result = load::save_global_page(
        &paths,
        &editor.form.edits(),
        editor.appearance.edits().as_ref(),
    );
    assert!(
        result.is_err() || result.as_ref().unwrap().failed.is_some(),
        "{result:?}"
    );
    assert!(!library_text(&paths).contains("dracula"));
}

#[test]
fn startup_applies_the_saved_appearance() {
    let (_root, paths) = setup();
    write_library(
        &paths,
        "[appearance]\ndark = \"cellar\"\nlight = \"cellar-light\"\n",
    );
    let mut app = App::boot(paths.clone()).0;
    assert_eq!(app.palette, palette("cellar"), "before the mode arrives");
    let _ = app.update(Message::ThemeChanged(Mode::Light));
    assert_eq!(app.palette, palette("cellar-light"));

    write_library(
        &paths,
        "[appearance]\nfollow_system = false\ntheme = \"catppuccin-latte\"\nimprove_readability = true\n",
    );
    let mut app = App::boot(paths.clone()).0;
    let readable = crate::theme::resolve(&app.appearance, Mode::Dark);
    assert_ne!(readable, palette("catppuccin-latte"));
    assert_eq!(app.palette, readable);
    let _ = app.update(Message::ThemeChanged(Mode::Dark));
    assert_eq!(app.palette, readable);
}

#[test]
fn an_unknown_saved_id_shows_its_fallback_as_chosen() {
    let (_root, paths) = setup();
    write_library(&paths, "[appearance]\ndark = \"custom:gone\"\n");
    let mut app = open_global_app(&paths);
    assert_eq!(app.palette, palette("midnight"));
    show_appearance(&mut app);
    // Picking the fallback writes a good id.
    pick(&mut app, Slot::Dark, "midnight");
    assert_eq!(page_of(&app).changes(), 1);
}

#[test]
fn a_hand_edit_to_library_toml_reloads_a_clean_tab() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    // General's edit stays through it, since config.toml didn't change.
    let _ = app.update(Message::Settings(Event::GlobalSwitch(
        Switch::Gamemode,
        true,
    )));
    write_library(&paths, "[appearance]\ndark = \"dracula\"\n");
    check_global(&mut app, &paths);
    let page = page_of(&app);
    assert!(!page.changed_on_disk);
    assert_eq!(page.appearance().unwrap().draft.dark, "dracula");
    assert_eq!(page.changes(), 1, "General's edit is kept");
    assert_eq!(app.palette, palette("dracula"));
    // Favorites and the library's view writing the file aren't a change.
    write_library(
        &paths,
        "[view]\nzoom = 120\n\n[appearance]\ndark = \"dracula\"\n",
    );
    check_global(&mut app, &paths);
    assert!(!page_of(&app).changed_on_disk);
    assert!(!page_of(&app).loading());
}

#[test]
fn a_hand_edit_under_appearance_edits_asks_for_reload() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    pick(&mut app, Slot::Dark, "cellar");
    write_library(&paths, "[appearance]\ndark = \"dracula\"\n");
    check_global(&mut app, &paths);
    assert!(page_of(&app).changed_on_disk);
    assert!(!page_of(&app).can_save());
    assert_eq!(app.palette, palette("cellar"));
    let _ = app.update(Message::Settings(Event::Reload));
    let generation = page_of(&app).generation;
    let _ = app.update(Message::Settings(Event::Loaded(
        generation,
        load::snapshot_global(&paths),
    )));
    assert!(!page_of(&app).is_dirty());
    assert_eq!(app.palette, palette("dracula"));
}

#[test]
fn appearance_edits_are_dropped_while_saving_and_on_the_environment_page() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    pick(&mut app, Slot::Dark, "cellar");
    let _ = app.update(Message::Settings(Event::Save));
    assert!(page_of(&app).locked());
    pick(&mut app, Slot::Dark, "dracula");
    assert_eq!(page_of(&app).appearance().unwrap().draft.dark, "cellar");

    write(&paths, GAME);
    let mut page = page(&paths);
    let _ = page.update(
        &paths,
        Event::Appearance(Edit::Theme(Slot::Dark, "dracula".into())),
    );
    assert!(!page.is_dirty());
    assert!(page.appearance().is_none());
}
