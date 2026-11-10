use iced::keyboard::{Key, Modifiers, key::Named};
use iced::theme::Mode;
use iced::widget::Id;

use super::*;
use crate::layout_probe::lay_out;
use crate::theme::{Palette, Preset};
use appearance::{Edit, FOLLOW_ID, READABILITY_ID, Slot};
use theme_colors::COPY_ID;
use wineo_core::library::Motion;

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
        None,
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
    expected.push(id(COPY_ID));
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
    expected.push(id(COPY_ID));
    expected.push(id(READABILITY_ID));
    assert_eq!(focusable_ids(&app, &paths), expected);
    assert_eq!(page_of(&app).changes(), 1);
}

#[test]
fn the_lists_offer_every_built_in_with_its_kind() {
    let choices = appearance::choices(&[]);
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
        None,
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
    let readable = crate::theme::resolve(&app.appearance, Mode::Dark, &[]);
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

#[test]
fn locked_rows_look_disabled() {
    for palette in crate::theme::PRESETS.map(|preset| preset.palette) {
        for chosen in [false, true] {
            let enabled = appearance::RowColors::new(chosen, true, palette);
            let locked = appearance::RowColors::new(chosen, false, palette);
            assert_ne!(locked.name, enabled.name, "chosen {chosen}");
            assert_ne!(locked.detail, enabled.detail, "chosen {chosen}");
        }
    }
    // The check rows' labels take the row's own text color, which changes
    // when the row is disabled.
    let palette = Palette::DARK;
    assert_ne!(
        crate::ui::list_row_style(palette, iced::widget::button::Status::Disabled).text_color,
        crate::ui::list_row_style(palette, iced::widget::button::Status::Active).text_color
    );
}

#[test]
fn a_save_locks_the_rows() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    pick(&mut app, Slot::Dark, "cellar");
    let _ = app.update(Message::Settings(Event::Save));
    let ids = focusable_ids(&app, &paths);
    assert!(!ids.contains(&id(FOLLOW_ID)), "{ids:?}");
    assert!(
        !ids.contains(&id(&Slot::Dark.row_id("midnight"))),
        "{ids:?}"
    );
}

#[test]
fn the_motion_dropdown_previews_live_and_save_writes_only_it() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    let shown = super::custom_themes::texts(&app, &paths);
    for label in ["Accessibility", "Motion"] {
        assert!(
            shown.iter().any(|text| text == label),
            "{label} in {shown:?}"
        );
    }
    assert!(!app.motion.reduced());

    edit(&mut app, Edit::Motion(Motion::Reduced));
    assert!(app.motion.reduced(), "the draft applies at once");
    assert_eq!(page_of(&app).changes(), 1);
    assert_eq!(library_text(&paths), "", "nothing written before Save");
    let _ = app.update(Message::Settings(Event::Revert));
    assert!(!app.motion.reduced(), "Revert undoes the preview");

    edit(&mut app, Edit::Motion(Motion::Reduced));
    let report = save(&mut app, &paths);
    assert_eq!(report.failed, None);
    assert_eq!(
        library_text(&paths),
        "schema_version = 1\n\n[appearance]\nmotion = \"reduced\"\n"
    );
    assert!(!page_of(&app).is_dirty());
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    assert!(app.settings.is_none());
    assert!(app.motion.reduced(), "the saved setting stays");
}

#[test]
fn startup_applies_the_saved_motion() {
    let (_root, paths) = setup();
    write_library(&paths, "[appearance]\nmotion = \"reduced\"\n");
    let app = App::boot(paths.clone()).0;
    assert!(app.motion.reduced(), "before any update");
    write_library(&paths, "[appearance]\nmotion = \"full\"\n");
    let mut app = App::boot(paths.clone()).0;
    let _ = app.update(Message::SystemMotion(true));
    assert!(!app.motion.reduced());
}

#[test]
fn the_interface_size_follows_the_draft_and_save_writes_only_it() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    let shown = super::custom_themes::texts(&app, &paths);
    assert!(
        shown.iter().any(|text| text == "Interface size"),
        "{shown:?}"
    );
    let main = app.main_window.unwrap();
    assert_eq!(app.scale_factor(main), 1.0);

    edit(&mut app, Edit::InterfaceSize(150));
    assert_eq!(app.scale_factor(main), 1.5, "the draft applies at once");
    assert_eq!(app.scale_factor(iced::window::Id::unique()), 1.0);
    assert_eq!(page_of(&app).changes(), 1);
    assert_eq!(library_text(&paths), "", "nothing written before Save");
    let _ = app.update(Message::Settings(Event::Revert));
    assert_eq!(app.scale_factor(main), 1.0, "Revert undoes the preview");

    edit(&mut app, Edit::InterfaceSize(175));
    let report = save(&mut app, &paths);
    assert_eq!(report.failed, None);
    assert_eq!(
        library_text(&paths),
        "schema_version = 1\n\n[appearance]\ninterface_size = 175\n"
    );
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    assert!(app.settings.is_none());
    assert_eq!(app.scale_factor(main), 1.75, "the saved size stays");
}

#[test]
fn discarding_the_draft_undoes_the_interface_size() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::InterfaceSize(200));
    let main = app.main_window.unwrap();
    assert_eq!(app.scale_factor(main), 2.0);
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    assert!(app.settings.is_some(), "asks before discarding");
    let _ = app.update(Message::Settings(Event::Discard));
    assert!(app.settings.is_none());
    assert_eq!(app.scale_factor(main), 1.0);
    assert_eq!(library_text(&paths), "");
}

/// Runs `task`, answering a `window::size` for `main` with `size` the way
/// iced would after a scale change, and returns the message the answer
/// became, if it asked.
fn answer_size(
    task: iced::Task<Message>,
    main: iced::window::Id,
    size: iced::Size,
) -> Option<Message> {
    use iced::futures::StreamExt;
    use iced_runtime::{Action, window::Action as Window};
    let mut stream = iced_runtime::task::into_stream(task)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut asked = false;
        while let Ok(Some(action)) =
            tokio::time::timeout(std::time::Duration::from_millis(500), stream.next()).await
        {
            match action {
                Action::Window(Window::GetSize(id, channel)) => {
                    assert_eq!(id, main);
                    let _ = channel.send(size);
                    asked = true;
                }
                Action::Output(message) if asked => return Some(message),
                _ => {}
            }
        }
        None
    })
}

#[test]
fn a_size_change_refreshes_the_window_size() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    let main = app.main_window.unwrap();
    let task = app.update(Message::Settings(Event::Appearance(Edit::InterfaceSize(
        150,
    ))));
    let smaller = iced::Size::new(800.0, 533.0);
    let message = answer_size(task, main, smaller).expect("asks for the window's size");
    assert!(
        matches!(message, Message::Window(id, iced::window::Event::Resized(size)) if id == main && size == smaller),
        "{message:?}"
    );
    let _ = app.update(message);
    assert_eq!(app.window_size, smaller);
    assert_eq!(
        app.viewport_width,
        smaller.width - crate::sidebar::WIDTH,
        "columns follow"
    );

    // Picking the size already in use asks for nothing.
    let task = app.update(Message::Settings(Event::Appearance(Edit::InterfaceSize(
        150,
    ))));
    assert!(answer_size(task, main, smaller).is_none());
}

#[test]
fn startup_opens_the_same_size_on_screen_at_any_interface_size() {
    use iced_runtime::{Action, window::Action as Window};
    let (_root, paths) = setup();
    for (size, open) in [
        (100u16, iced::Size::new(1200.0, 800.0)),
        (200, iced::Size::new(600.0, 400.0)),
        (125, iced::Size::new(960.0, 640.0)),
    ] {
        write_library(&paths, &format!("[appearance]\ninterface_size = {size}\n"));
        let (app, boot) = App::boot(paths.clone());
        let main = app.main_window.unwrap();
        assert_eq!(app.scale_factor(main), f32::from(size) / 100.0);
        assert_eq!(app.window_size, open, "{size}%");
        assert_eq!(app.viewport_width, open.width - crate::sidebar::WIDTH);
        assert_eq!(app.viewport_height, open.height - 50.0);
        let mut stream = iced_runtime::task::into_stream(boot).unwrap();
        let action = iced::futures::executor::block_on(iced::futures::StreamExt::next(&mut stream));
        let Some(Action::Window(Window::Open(_, settings, _))) = action else {
            panic!("boot's first action opens a window");
        };
        assert_eq!(settings.size, open, "{size}%");
    }
}

#[test]
fn a_hand_edited_size_between_steps_is_used_and_listed() {
    let (_root, paths) = setup();
    write_library(&paths, "[appearance]\ninterface_size = 130\n");
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    assert_eq!(app.scale_factor(app.main_window.unwrap()), 1.3);
    let labels: Vec<String> = appearance::size_choices(130)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        labels,
        ["100%", "110%", "125%", "130%", "150%", "175%", "200%"]
    );
    assert_eq!(appearance::size_choices(150).len(), 6);
    // Out of range in the file clamps.
    write_library(&paths, "[appearance]\ninterface_size = 900\n");
    let app = App::boot(paths.clone()).0;
    assert_eq!(app.scale_factor(app.main_window.unwrap()), 2.0);
}

use crate::fonts::fixtures::{WIDE, install_wide};
use crate::fonts::{BUILT_IN, live};

fn font_form(app: &App) -> &appearance::Form {
    page_of(app).appearance().unwrap()
}

#[test]
fn the_font_dropdown_previews_live_and_save_writes_only_it() {
    install_wide();
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    assert_eq!(
        live::take(),
        Vec::<&str>::new(),
        "the bundle needs no remap"
    );
    let shown = super::custom_themes::texts(&app, &paths);
    for label in ["Font", appearance::BUILT_IN_LABEL] {
        assert!(
            shown.iter().any(|text| text == label),
            "{label} in {shown:?}"
        );
    }

    edit(&mut app, Edit::FontOpen);
    assert!(page_of(&app).menu_open, "Escape and the sidebar wait");
    let picker = font_form(&app).font_picker.as_ref().unwrap();
    let rows = picker.rows();
    assert_eq!(rows[0], (appearance::BUILT_IN_LABEL, None));
    assert!(rows.contains(&(WIDE, Some(WIDE))));
    assert!(!rows.iter().any(|(label, _)| *label == BUILT_IN));
    edit(&mut app, Edit::FontQuery("TEST wide".into()));
    let rows = font_form(&app).font_picker.as_ref().unwrap().rows();
    assert_eq!(rows, [(WIDE, Some(WIDE))]);
    edit(&mut app, Edit::FontSubmit);
    assert!(font_form(&app).font_picker.is_none());
    assert!(!page_of(&app).menu_open);
    assert_eq!(font_form(&app).draft.font.as_deref(), Some(WIDE));
    assert_eq!(live::take(), [WIDE], "the draft applies at once");
    assert_eq!(font_form(&app).font_in_use, WIDE);
    let shown = super::custom_themes::texts(&app, &paths);
    assert!(shown.iter().any(|text| text == WIDE), "{shown:?}");
    assert_eq!(page_of(&app).changes(), 1);
    assert_eq!(library_text(&paths), "", "nothing written before Save");

    let _ = app.update(Message::Settings(Event::Revert));
    assert_eq!(live::take(), [BUILT_IN], "Revert undoes the remap");

    edit(&mut app, Edit::Font(Some(WIDE.to_owned())));
    assert_eq!(live::take(), [WIDE]);
    let report = save(&mut app, &paths);
    assert_eq!(report.failed, None);
    assert_eq!(
        library_text(&paths),
        format!("schema_version = 1\n\n[appearance]\nfont = \"{WIDE}\"\n")
    );
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    assert!(app.settings.is_none());
    assert_eq!(live::take(), Vec::<&str>::new(), "the saved font stays");
}

#[test]
fn discarding_the_draft_undoes_the_font() {
    install_wide();
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::Font(Some(WIDE.to_owned())));
    assert_eq!(live::take(), [WIDE]);
    let _ = app.update(Message::Section(crate::sidebar::Section::Library));
    let _ = app.update(Message::Settings(Event::Discard));
    assert!(app.settings.is_none());
    assert_eq!(live::take(), [BUILT_IN]);
    assert_eq!(library_text(&paths), "");
}

#[test]
fn startup_applies_the_saved_font() {
    install_wide();
    let (_root, paths) = setup();
    write_library(&paths, &format!("[appearance]\nfont = \"{WIDE}\"\n"));
    live::take();
    let _app = App::boot(paths.clone()).0;
    assert_eq!(live::take(), [WIDE], "before any update");
}

#[test]
fn a_missing_font_shows_the_bundle_and_the_draft_keeps_its_name() {
    let (_root, paths) = setup();
    let text = "[appearance]\nfont = \"Not Installed Anywhere\"\n";
    write_library(&paths, text);
    live::take();
    let mut app = open_global_app(&paths);
    assert_eq!(live::take(), Vec::<&str>::new(), "the bundle stays in use");
    assert_eq!(library_text(&paths), text, "nothing rewrites the file");
    show_appearance(&mut app);
    let form = font_form(&app);
    assert_eq!(form.draft.font.as_deref(), Some("Not Installed Anywhere"));
    assert_eq!(form.font_in_use, BUILT_IN);
    let shown = super::custom_themes::texts(&app, &paths);
    assert!(shown.iter().any(|text| text == appearance::BUILT_IN_LABEL));

    // Saving another change keeps the name it doesn't show.
    edit(&mut app, Edit::Motion(Motion::Reduced));
    assert_eq!(page_of(&app).changes(), 1);
    let report = save(&mut app, &paths);
    assert_eq!(report.failed, None);
    let saved = library_text(&paths);
    assert!(
        saved.contains("font = \"Not Installed Anywhere\"")
            && saved.contains("motion = \"reduced\""),
        "{saved}"
    );

    // Picking the built-in font is a change, and removes the name.
    edit(&mut app, Edit::Font(None));
    assert_eq!(page_of(&app).changes(), 1);
    save(&mut app, &paths);
    assert!(!library_text(&paths).contains("font"));
}

#[test]
fn each_font_row_draws_in_its_own_family() {
    use crate::layout_probe::{bounds_at, lay_out};
    install_wide();
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::FontOpen);
    let picker = font_form(&app).font_picker.clone().unwrap();
    let rows = picker.rows();
    let wide = rows.iter().position(|row| row.0 == WIDE).unwrap();
    let laid = lay_out(
        appearance::font_popup(&picker, BUILT_IN, Palette::DARK),
        iced::Size::new(260.0, 2000.0),
    );
    // The popup's column, its list, the rows, and each row's label.
    let label = |index: usize| bounds_at(&laid.node, &[0, 1, 0, index, 0]).width;
    let own = label(wide);
    let in_noto = crate::tile_title::line_width(WIDE, 14.0, iced::Font::with_name(BUILT_IN));
    assert!(own > in_noto * 1.8, "{own} against {in_noto}");
    assert_eq!(
        label(0),
        crate::tile_title::line_width(appearance::BUILT_IN_LABEL, 14.0, iced::Font::DEFAULT)
    );
}

#[test]
fn a_save_closes_the_font_dropdown_with_the_other_menus() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::InterfaceSize(125));
    edit(&mut app, Edit::FontOpen);
    assert!(page_of(&app).menu_open);
    save(&mut app, &paths);
    assert!(font_form(&app).font_picker.is_none());
    assert!(!page_of(&app).menu_open);
}

#[test]
fn a_font_switch_and_the_leave_prompt_keep_the_page_scrolled() {
    use crate::layout_probe::{lay_out, relay_out};
    install_wide();
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    let size = iced::Size::new(900.0, 500.0);
    let mut laid = lay_out(app.view(), size);
    laid.run(Box::new(
        iced::advanced::widget::operation::scrollable::scroll_to(
            Id::new(super::super::view::SCROLL_ID),
            iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: 400.0 }.into(),
        ),
    ));
    assert_eq!(laid.offsets(), [400.0]);
    let mut tree = laid.into_tree();
    for (step, message) in [
        ("open", Message::Settings(Event::Appearance(Edit::FontOpen))),
        (
            "pick",
            Message::Settings(Event::Appearance(Edit::Font(Some(WIDE.to_owned())))),
        ),
        ("prompt", Message::Section(crate::sidebar::Section::Library)),
        ("keep editing", Message::Settings(Event::KeepEditing)),
    ] {
        let _ = app.update(message);
        let mut laid = relay_out(app.view(), tree, size);
        assert_eq!(laid.offsets(), [400.0], "after {step}");
        tree = laid.into_tree();
    }
    assert!(!page_of(&app).leave_prompt && page_of(&app).is_dirty());
}

#[test]
fn the_font_dropdown_draws_the_pick_lists_arrow() {
    use crate::layout_probe::render;
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    let size = iced::Size::new(view::CONTROL_WIDTH, 40.0);
    let width = view::CONTROL_WIDTH as usize;
    // The right end, past both labels, where each draws its arrow.
    let arrow = |pixels: Vec<u8>| -> Vec<u8> {
        pixels
            .chunks(width * 4)
            .flat_map(|line| line[(width - 32) * 4..].to_vec())
            .collect()
    };
    let form = font_form(&app);
    let pick_list = arrow(render(
        &mut appearance::size_choice(100, false, Palette::DARK),
        size,
    ));
    let font = arrow(render(
        &mut appearance::font_choice(form, false, Palette::DARK),
        size,
    ));
    // Something is drawn there, not just the background.
    assert!(pick_list.chunks(4).any(|pixel| *pixel != pick_list[..4]));
    assert_eq!(font, pick_list);
    // The arrow lies over the whole control, and presses still reach it.
    assert_eq!(
        crate::layout_probe::click_at(
            appearance::font_choice(form, false, Palette::DARK),
            size,
            &[]
        ),
        [format!(
            "{:?}",
            Message::Settings(Event::Appearance(Edit::FontOpen))
        )]
    );
}

fn font_picker(app: &App) -> &appearance::FontPicker {
    font_form(app).font_picker.as_ref().unwrap()
}

fn highlighted(app: &App) -> Option<&'static str> {
    let picker = font_picker(app);
    picker.rows().get(picker.highlight).map(|row| row.0)
}

#[test]
fn the_font_highlight_starts_on_the_chosen_family_and_the_arrows_wrap() {
    install_wide();
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::FontOpen);
    assert_eq!(highlighted(&app), Some(appearance::BUILT_IN_LABEL));
    edit(&mut app, Edit::FontClose);

    edit(&mut app, Edit::Font(Some(WIDE.to_owned())));
    edit(&mut app, Edit::FontOpen);
    assert_eq!(highlighted(&app), Some(WIDE));
    let count = font_picker(&app).rows().len();
    let wide = font_picker(&app).highlight;

    edit(&mut app, Edit::FontMove(false));
    assert_eq!(font_picker(&app).highlight, (wide + 1) % count);
    edit(&mut app, Edit::FontMove(true));
    assert_eq!(font_picker(&app).highlight, wide);
    for _ in 0..=wide {
        edit(&mut app, Edit::FontMove(true));
    }
    assert_eq!(
        font_picker(&app).highlight,
        count - 1,
        "Up from the top wraps"
    );
    edit(&mut app, Edit::FontMove(false));
    assert_eq!(font_picker(&app).highlight, 0, "Down from the bottom wraps");

    // Enter picks the highlighted row, not the first.
    edit(&mut app, Edit::FontMove(true));
    let last = font_picker(&app).rows()[count - 1].1;
    edit(&mut app, Edit::FontSubmit);
    assert!(font_form(&app).font_picker.is_none());
    assert_eq!(font_form(&app).draft.font.as_deref(), last);
}

#[test]
fn searching_moves_the_font_highlight_to_the_first_match() {
    install_wide();
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::FontOpen);
    edit(&mut app, Edit::FontMove(true));
    assert_ne!(font_picker(&app).highlight, 0);
    edit(&mut app, Edit::FontQuery("test".into()));
    assert_eq!(font_picker(&app).highlight, 0);
    assert_eq!(
        highlighted(&app),
        font_picker(&app).rows().first().map(|row| row.0)
    );
    // With nothing matching, the arrows and Enter do nothing.
    edit(&mut app, Edit::FontQuery("no family is called this".into()));
    edit(&mut app, Edit::FontMove(false));
    assert_eq!(highlighted(&app), None);
    edit(&mut app, Edit::FontSubmit);
    assert!(font_form(&app).font_picker.is_some());
    assert_eq!(font_form(&app).draft.font, None);
}

#[test]
fn the_highlighted_font_row_scrolls_into_the_lists_view() {
    use crate::focus_button::run_task;
    use crate::layout_probe::{bounds_at, lay_out, relay_out, within};
    const FAMILIES: [&str; 30] = [
        "Family 01",
        "Family 02",
        "Family 03",
        "Family 04",
        "Family 05",
        "Family 06",
        "Family 07",
        "Family 08",
        "Family 09",
        "Family 10",
        "Family 11",
        "Family 12",
        "Family 13",
        "Family 14",
        "Family 15",
        "Family 16",
        "Family 17",
        "Family 18",
        "Family 19",
        "Family 20",
        "Family 21",
        "Family 22",
        "Family 23",
        "Family 24",
        "Family 25",
        "Family 26",
        "Family 27",
        "Family 28",
        "Family 29",
        "Family 30",
    ];
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::FontOpen);
    let picker = app
        .settings
        .as_mut()
        .and_then(|page| page.appearance_mut())
        .and_then(|form| form.font_picker.as_mut())
        .unwrap();
    picker.families = FAMILIES.to_vec();
    let size = iced::Size::new(260.0, 2000.0);
    let popup = |app: &App| appearance::font_popup(font_picker(app), BUILT_IN, Palette::DARK);
    // The list's viewport, and the highlighted row in the window.
    let shown = |laid: &crate::layout_probe::Laid<'_, Message>, app: &App| {
        let viewport = bounds_at(&laid.node, &[0, 1]);
        let row = bounds_at(&laid.node, &[0, 1, 0, font_picker(app).highlight]);
        (viewport, row)
    };

    let mut laid = lay_out(popup(&app), size);
    assert_eq!(laid.offsets(), [0.0]);
    let task = app.update(Message::Settings(Event::Appearance(Edit::FontMove(true))));
    assert_eq!(highlighted(&app), Some("Family 30"));
    let mut laid = relay_out(popup(&app), laid.into_tree(), size);
    run_task(&mut laid, task);
    let offset = laid.offsets()[0];
    let (viewport, row) = shown(&laid, &app);
    let row = iced::Rectangle {
        y: row.y - offset,
        ..row
    };
    // The scrollable draws at a whole pixel offset.
    let viewport = viewport.expand(1.0);
    assert!(
        offset > 0.0 && within(row, viewport),
        "{row:?} in {viewport:?}"
    );

    let task = app.update(Message::Settings(Event::Appearance(Edit::FontMove(false))));
    assert_eq!(font_picker(&app).highlight, 0);
    let mut laid = relay_out(popup(&app), laid.into_tree(), size);
    run_task(&mut laid, task);
    assert_eq!(laid.offsets(), [0.0], "the first row goes back to the top");
}

#[test]
fn the_highlighted_font_row_draws_the_focus_outline_not_a_tint() {
    use crate::layout_probe::{bounds_at, layout, render};
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::FontOpen);
    let mut picker = font_picker(&app).clone();
    picker.families = vec!["Family 01", "Family 02", "Family 03"];
    let size = iced::Size::new(260.0, 400.0);
    let draw = |highlight: usize| {
        let picker = appearance::FontPicker {
            highlight,
            ..picker.clone()
        };
        let mut popup = appearance::font_popup(&picker, BUILT_IN, Palette::DARK);
        let node = layout(&mut popup, size);
        (render(&mut popup, size), node)
    };
    let pixel = |pixels: &[u8], x: f32, y: f32| {
        let at = (y as usize * size.width as usize + x as usize) * 4;
        pixels[at..at + 4].to_vec()
    };
    // The last row, far enough that its outline misses rows 0 and 1.
    let (away, node) = draw(3);
    // Row 0 is the chosen built-in font, row 1 a plain one.
    for row in [0, 1] {
        let bounds = bounds_at(&node, &[0, 1, 0, row]);
        let (on, _) = draw(row);
        let mid = bounds.x + bounds.width / 2.0;
        let edge = bounds.y + bounds.height + 1.0;
        assert_ne!(
            pixel(&on, mid, edge),
            pixel(&away, mid, edge),
            "row {row}'s outline"
        );
        let inside = bounds.y + bounds.height / 2.0;
        let left = bounds.x + 4.0;
        assert_eq!(
            pixel(&on, left, inside),
            pixel(&away, left, inside),
            "row {row}'s fill"
        );
    }
}

#[test]
fn revert_closes_the_font_dropdown_and_unblocks_the_page() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::InterfaceSize(125));
    edit(&mut app, Edit::FontOpen);
    assert!(page_of(&app).navigation_blocked());
    let _ = app.update(Message::Settings(Event::Revert));
    assert!(font_form(&app).font_picker.is_none());
    assert!(!page_of(&app).menu_open);
    assert!(!page_of(&app).navigation_blocked());
}

#[test]
fn switching_tabs_closes_the_font_dropdown() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::FontOpen);
    let _ = app.update(Message::Settings(Event::Tab(Tab::General)));
    assert!(font_form(&app).font_picker.is_none());
    assert!(!page_of(&app).navigation_blocked());
    show_appearance(&mut app);
    assert!(
        font_form(&app).font_picker.is_none(),
        "it comes back closed"
    );
}

#[test]
fn starting_a_save_closes_the_font_dropdown() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::InterfaceSize(125));
    edit(&mut app, Edit::FontOpen);
    // Before the save's task runs, so a refused save can't show it again.
    let _ = app.update(Message::Settings(Event::Save));
    assert!(page_of(&app).saving);
    assert!(font_form(&app).font_picker.is_none());
    assert!(!page_of(&app).menu_open);
}

#[test]
fn an_open_font_dropdown_holds_navigation_without_the_menu_flag() {
    let (_root, paths) = setup();
    let mut app = open_global_app(&paths);
    show_appearance(&mut app);
    edit(&mut app, Edit::FontOpen);
    app.settings.as_mut().unwrap().menu_open = false;
    assert!(page_of(&app).navigation_blocked());
}
