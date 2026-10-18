use crate::flow::import_grid::measure;
use crate::test_script::write_script;
use iced::keyboard::key::Named;
use std::ffi::OsString;
use std::fs;

use tempfile::tempdir;

use super::view::{hook_marker, shared_prefix_note, skip_reason};
use super::work::{PROGRESS_INTERVAL, grouped, runner_line};
use super::*;
use crate::flow::import_grid::import_label;
use std::time::Duration;
use wineo_core::gamescope::Filter;
use wineo_core::heroic::{HeroicSharedPrefix, SettingsReview};
use wineo_core::heroic_import::{Entry, SkipReason, Warning};
use wineo_core::import;
use wineo_core::runners::Runners;

fn paths(root: &std::path::Path) -> Paths {
    let home = root.to_owned();
    Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
        .unwrap()
}

fn native_fixture(root: &std::path::Path) -> (HeroicSource, PathBuf) {
    let heroic = root.join("heroic");
    fs::create_dir_all(heroic.join("sideload_apps")).unwrap();
    fs::create_dir_all(heroic.join("GamesConfig")).unwrap();
    fs::create_dir_all(heroic.join("store")).unwrap();
    let exe = root.join("games/original");
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    write_script(&exe, "#!/bin/sh\n");
    fs::write(
        heroic.join("sideload_apps/library.json"),
        format!(
            r#"{{"games":[{{"app_name":"dusk","title":"Dusklight","runner":"sideload","install":{{"platform":"linux","executable":"{}"}}}}]}}"#,
            exe.display()
        ),
    )
    .unwrap();
    fs::write(
        heroic.join("GamesConfig/dusk.json"),
        r#"{"dusk":{"launcherArgs":"--old"}}"#,
    )
    .unwrap();
    (
        HeroicSource::from_config_dir_with_home(heroic, root.to_owned()),
        exe,
    )
}

#[test]
fn native_review_edits_are_read_only_until_import() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let (source, _) = native_fixture(root.path());
    let global = GlobalConfig::load(&paths).unwrap();
    let listing = heroic_import::listing(&paths, &global, &source).unwrap();
    let new_exe = root.path().join("games/changed");
    write_script(&new_exe, "#!/bin/sh\n");
    let before = fs::read(source.config_dir().join("GamesConfig/dusk.json")).unwrap();
    let review = plan_review(
        &paths,
        &listing,
        Some(0),
        &Draft {
            name: "changed".to_owned(),
            executable: new_exe.display().to_string(),
            arguments: vec!["--name".to_owned(), "two words".to_owned()],
            working_directory: root.path().join("games").display().to_string(),
            ..Draft::default()
        },
    )
    .unwrap();
    let Review::Single { game, .. } = review else {
        panic!("expected native plan")
    };
    let Game::Native(plan) = game.as_ref() else {
        panic!("expected native game")
    };
    let target = &plan.settings().targets["game"];
    assert_eq!(target.exe, new_exe.display().to_string());
    assert_eq!(target.args, ["--name", "two words"]);
    assert_eq!(target.cwd.as_deref(), root.path().join("games").to_str());
    assert_eq!(
        fs::read(source.config_dir().join("GamesConfig/dusk.json")).unwrap(),
        before
    );
    assert!(!paths.environment_config("changed").exists());
}

#[test]
fn native_launch_controls_map_to_the_reviewed_plan() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let (source, exe) = native_fixture(root.path());
    let global = GlobalConfig::load(&paths).unwrap();
    let listing = heroic_import::listing(&paths, &global, &source).unwrap();
    let before = fs::read(source.config_dir().join("GamesConfig/dusk.json")).unwrap();
    let mut draft = Draft {
        name: "changed".to_owned(),
        executable: exe.display().to_string(),
        arguments: vec!["--old".to_owned()],
        environment: vec!["NEW=value".to_owned()],
        environment_dirty: true,
        gamemode: true,
        gamemode_dirty: true,
        mangohud: true,
        mangohud_dirty: true,
        gamescope: ScopeDraft::from_setting(Some(GamescopeSetting::On(Gamescope::default()))),
        ..Draft::default()
    };
    draft.gamescope.numbers[0] = "1280".to_owned();
    draft.gamescope.dirty = true;
    let Review::Single { game, .. } = plan_review(&paths, &listing, Some(0), &draft).unwrap()
    else {
        panic!("expected single review");
    };
    let Game::Native(plan) = game.as_ref() else {
        panic!("expected native plan");
    };
    let settings = plan.settings();
    assert_eq!(settings.env["NEW"], "value");
    assert_eq!(settings.gamemode, Some(true));
    assert_eq!(settings.mangohud, Some(true));
    assert_eq!(
        settings
            .gamescope
            .and_then(|setting| setting.on().cloned())
            .unwrap()
            .width,
        Some(1280)
    );
    assert_eq!(
        fs::read(source.config_dir().join("GamesConfig/dusk.json")).unwrap(),
        before
    );
}

#[test]
fn gamescope_toggle_preserves_edited_options() {
    let root = tempdir().unwrap();
    let (mut page, _) = Page::new(paths(root.path()));
    page.draft.gamescope = ScopeDraft::from_setting(Some(GamescopeSetting::On(Gamescope {
        filter: Some(Filter::Fsr),
        grab_cursor: true,
        ..Gamescope::default()
    })));
    let _ = page.update(Event::GamescopeEnabled(false));
    let _ = page.update(Event::GamescopeEnabled(true));
    let scope = page.draft.gamescope.resolved().unwrap().unwrap();
    let scope = scope.on().unwrap();
    assert_eq!(scope.filter, Some(Filter::Fsr));
    assert!(scope.grab_cursor);
}

#[test]
fn stale_discovery_reply_does_not_replace_selected_source() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let (mut page, _) = Page::new(paths);
    let stale = page.generation;
    page.next_generation();
    page.sources
        .push(HeroicSource::from_config_dir(root.path().join("chosen")));
    let _ = page.update(Event::SourcesLoaded(stale, Ok(Vec::new())));
    assert_eq!(page.sources.len(), 1);
}

#[test]
fn cancel_and_back_preserve_review_draft() {
    let root = tempdir().unwrap();
    let (mut page, _) = Page::new(paths(root.path()));
    page.step = Step::Review;
    page.draft.name = "my-game".to_owned();
    let _ = page.back();
    assert_eq!(page.step, Step::Details);
    assert_eq!(page.draft.name, "my-game");
    page.step = Step::Working;
    let token = Token::new();
    page.token = Some(token.clone());
    let _ = page.update(Event::Cancel);
    assert!(token.is_canceled());
    assert!(page.busy());
    let _ = page.update(Event::Force);
    assert!(token.is_forced());
}

#[test]
fn stale_picker_replies_cannot_change_source_or_executable() {
    let root = tempdir().unwrap();
    let (mut page, _) = Page::new(paths(root.path()));
    page.picker_sequence = 4;
    let _ = page.update(Event::PickedSource(3, Ok(Some(root.path().join("stale")))));
    assert!(page.sources.is_empty());
    page.step = Step::Details;
    page.draft.executable = "chosen.exe".to_owned();
    let _ = page.update(Event::PickedExecutable(
        3,
        Ok(Some(root.path().join("stale.exe"))),
    ));
    assert_eq!(page.draft.executable, "chosen.exe");
    let _ = page.update(Event::PickedWorkingDirectory(
        3,
        Ok(Some(root.path().join("stale-dir"))),
    ));
    assert!(page.draft.working_directory.is_empty());
}

#[test]
fn argument_chips_preserve_spaces_quotes_and_empty_values() {
    let arguments = vec![
        "two words".to_owned(),
        "it's ready".to_owned(),
        String::new(),
    ];
    assert_eq!(
        wineo_core::heroic::parse_launcher_args(&quote_args(&arguments)).unwrap(),
        arguments
    );
}

#[test]
fn changed_heroic_settings_refuse_registration() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    let (source, exe) = native_fixture(root.path());
    let global = GlobalConfig::load(&paths).unwrap();
    let listing = heroic_import::listing(&paths, &global, &source).unwrap();
    let review = plan_review(
        &paths,
        &listing,
        Some(0),
        &Draft {
            name: "changed".to_owned(),
            executable: exe.display().to_string(),
            arguments: vec!["--old".to_owned()],
            ..Draft::default()
        },
    )
    .unwrap();
    let Review::Single { game, .. } = review else {
        panic!("expected single plan")
    };
    let settings = source.config_dir().join("GamesConfig/dusk.json");
    fs::write(&settings, r#"{"dusk":{"launcherArgs":"--new"}}"#).unwrap();
    let error = apply_game(&paths, &global, &source, &game, &Token::new(), |_| {}).unwrap_err();
    assert!(error.is::<heroic_import::ApplyError>());
    assert!(!paths.environment_config("changed").exists());
    assert_eq!(
        fs::read_to_string(settings).unwrap(),
        r#"{"dusk":{"launcherArgs":"--new"}}"#
    );
}

// -- the poster grid --

const DUSK: usize = 0;
const DAWN: usize = 1;
const NOON: usize = 2;
const OLD: usize = 3;
const STORE: usize = 4;

/// Native games Dusk (a PNG poster), Dawn (a poster with a JPEG signature
/// but a corrupt body),
/// Noon (no poster), and Old, then Store, which Heroic's store owns.
fn grid_fixture(root: &std::path::Path) -> (Paths, HeroicListing) {
    let heroic = root.join("heroic");
    for dir in ["sideload_apps", "GamesConfig", "store", "images-cache"] {
        fs::create_dir_all(heroic.join(dir)).unwrap();
    }
    let source = HeroicSource::from_config_dir_with_home(heroic.clone(), root.to_owned());
    let mut games = Vec::new();
    for (id, title) in [
        ("dusk", "Dusk"),
        ("dawn", "Dawn"),
        ("noon", "Noon"),
        ("old", "Old"),
    ] {
        let exe = root.join("games").join(id);
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        write_script(&exe, "#!/bin/sh\n");
        fs::write(
            heroic.join(format!("GamesConfig/{id}.json")),
            format!(r#"{{"{id}":{{}}}}"#),
        )
        .unwrap();
        let art = if id == "noon" {
            String::new()
        } else {
            format!(r#""art_square":"https://cdn.example.com/{id}.jpg","#)
        };
        games.push(format!(
            r#"{{"app_name":"{id}","title":"{title}","runner":"sideload",{art}"install":{{"platform":"linux","executable":"{}"}}}}"#,
            exe.display()
        ));
    }
    games.push(
        r#"{"app_name":"store","title":"Store","runner":"legendary","install":{"platform":"Windows","executable":"x.exe"}}"#
            .to_owned(),
    );
    fs::write(
        heroic.join("sideload_apps/library.json"),
        format!(r#"{{"games":[{}]}}"#, games.join(",")),
    )
    .unwrap();
    ::image::RgbImage::from_pixel(4, 6, ::image::Rgb([40, 100, 180]))
        .save_with_format(
            source.cached_image("https://cdn.example.com/dusk.jpg"),
            ::image::ImageFormat::Png,
        )
        .unwrap();
    fs::write(
        source.cached_image("https://cdn.example.com/dawn.jpg"),
        b"\xff\xd8\xff\xe0 corrupt body",
    )
    .unwrap();
    let paths = paths(root);
    let global = GlobalConfig::load(&paths).unwrap();
    let listing = heroic_import::listing(&paths, &global, &source).unwrap();
    (paths, listing)
}

fn grid_page(root: &std::path::Path) -> Page {
    let (paths, listing) = grid_fixture(root);
    let (mut page, _) = Page::new(paths);
    page.step = Step::Games;
    page.listing = Some(listing);
    // A finished listing reply clears the "Reading Heroic games" status.
    page.status.clear();
    page.already.insert("old".to_owned(), "old".to_owned());
    page
}

fn chosen(page: &Page) -> Vec<&str> {
    page.grid.chosen.iter().map(String::as_str).collect()
}

/// Every file under `dir` with its bytes, to show nothing there changed.
fn tree(dir: &std::path::Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(tree(&path));
        } else {
            files.insert(path.clone(), fs::read(&path).unwrap());
        }
    }
    files
}

#[test]
fn only_keyboard_focus_draws_the_ring() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    let none = keyboard::Modifiers::empty();
    let _ = page.update(Event::Toggle(DUSK));
    assert_eq!(page.grid.focus, Some(DUSK));
    assert!(!page.grid.focus_ring(DUSK));
    let _ = page.key(&Key::Named(Named::ArrowRight), none, 900.0);
    let focus = page.grid.focus.unwrap();
    assert!(page.grid.focus_ring(focus));
    let _ = page.update(Event::Toggle(focus));
    assert!(!page.grid.focus_ring(focus));
    // Space after a click is keyboard use, so the ring comes back.
    let _ = page.key(&Key::Named(Named::Space), none, 900.0);
    assert!(page.grid.focus_ring(focus));
}

/// A page at each step, filled in the way a real import fills it.
fn staged_pages(root: &std::path::Path) -> Vec<(&'static str, Page)> {
    const LONG: &str = "Grand Theft Auto IV: The Complete Edition and Episodes from Liberty City";
    let page = |name: &str| {
        let mut page = grid_page(&root.join(name));
        if let Some(listing) = &mut page.listing {
            listing.candidates[DUSK].title = LONG.to_owned();
        }
        page
    };
    let mut pages = Vec::new();
    let mut sources = page("sources");
    sources.step = Step::Sources;
    pages.push(("sources", sources));

    let mut games = page("games");
    let _ = games.update(Event::Toggle(DUSK));
    let _ = games.update(Event::Toggle(NOON));
    let _ = games.key(
        &Key::Named(Named::ArrowRight),
        keyboard::Modifiers::empty(),
        900.0,
    );
    pages.push(("games", games));

    let mut single = page("single");
    open_single(&mut single, DUSK);
    pages.push(("single details", single));
    let mut single = page("single-review");
    open_single(&mut single, DUSK);
    let listing = single.listing.clone().unwrap();
    let review = plan_review(&single.paths, &listing, single.selected, &single.draft).unwrap();
    let _ = single.update(Event::Planned(single.generation, Ok(Box::new(review))));
    assert_eq!(single.step, Step::Review);
    pages.push(("single review", single));

    let mut batch = page("batch");
    let _ = batch.update(Event::Toggle(DUSK));
    let _ = batch.update(Event::Toggle(NOON));
    let _ = batch.update(Event::ImportSelected);
    assert!(batch.draft.batch);
    pages.push(("batch details", batch));
    let mut batch = page("batch-review");
    let _ = batch.update(Event::Toggle(DUSK));
    let _ = batch.update(Event::Toggle(NOON));
    let _ = batch.update(Event::ImportSelected);
    let listing = batch.listing.clone().unwrap();
    let review = plan_review(&batch.paths, &listing, None, &batch.draft).unwrap();
    let _ = batch.update(Event::Planned(batch.generation, Ok(Box::new(review))));
    assert_eq!(batch.step, Step::Review);
    pages.push(("batch review", batch));

    let results = || {
        vec![
            (
                "grand-theft-auto-iv-the-complete-edition".to_owned(),
                Ok(vec!["Desktop entry: the menu folder couldn't be written".to_owned()]),
            ),
            (
                "noon".to_owned(),
                Err("Heroic no longer lists the install folder for this game, so it can't be imported".to_owned()),
            ),
        ]
    };
    let mut working = page("working");
    working.step = Step::Working;
    working.token = Some(Token::new());
    working.results = results();
    pages.push(("working", working));
    let mut finished = page("finished");
    finished.step = Step::Finished;
    finished.results = results();
    pages.push(("finished", finished));
    pages
}

#[test]
fn every_step_sits_inside_the_dialog_padding() {
    use crate::layout_probe::{bounds_at, probe, within};
    let root = tempdir().unwrap();
    let mut pages = staged_pages(root.path());
    // The smallest dialog the window allows, a small one, and the widest.
    for (width, height) in [(280.0, 240.0), (360.0, 280.0), (900.0, 720.0)] {
        let inset = iced::Rectangle::new(
            iced::Point::new(24.0, 24.0),
            iced::Size::new(width - 48.0, height - 48.0),
        );
        for (label, page) in &mut pages {
            // Back only asks to discard the draft from Sources.
            let prompts: &[bool] = if *label == "sources" {
                &[false, true]
            } else {
                &[false]
            };
            for &leave_prompt in prompts {
                page.leave_prompt = leave_prompt;
                let mut view = page.view_with_key_availability(Palette::DARK, width, false);
                let probe = probe(&mut view, iced::Size::new(width, height));
                let node = &probe.node;
                let spilled = probe.outside(inset);
                assert_eq!(probe.scrollables.len(), 1, "one scrolling region");
                assert!(
                    spilled.is_empty(),
                    "{label} at {width}x{height}, prompt {leave_prompt}: {spilled:?}"
                );
                let back = bounds_at(node, &[0, if leave_prompt { 2 } else { 0 }, 0]);
                assert!(within(back, inset), "{label}: {back:?}");
            }
        }
    }
}

#[test]
fn a_short_dialog_keeps_the_actions_in_frame_and_the_grid_scrolling() {
    use crate::layout_probe::{bounds_at, probe, within};
    let root = tempdir().unwrap();
    let page = grid_page(root.path());
    for (width, height) in [(280.0, 240.0), (360.0, 200.0), (900.0, 240.0)] {
        let inset = iced::Rectangle::new(
            iced::Point::new(24.0, 24.0),
            iced::Size::new(width - 48.0, height - 48.0),
        );
        let mut view = page.view_with_key_availability(Palette::DARK, width, false);
        let probe = probe(&mut view, iced::Size::new(width, height));
        assert_eq!(probe.scrollables.len(), 1, "one scrolling region");
        let (viewport, content) = probe.scrollables[0];
        assert!(viewport.height > 0.0, "{width}x{height}: {viewport:?}");
        // The title and notes are the top of the scrolled content.
        assert!(
            (content.y - viewport.y).abs() < 0.01 && content.height > viewport.height,
            "{width}x{height}: {viewport:?} {content:?}"
        );
        let actions = bounds_at(&probe.node, &[0, 1, 1]);
        assert!(within(actions, inset), "{width}x{height}: {actions:?}");
        assert!(
            actions.y >= viewport.y + viewport.height - 0.01,
            "{width}x{height}: {actions:?} under {viewport:?}"
        );
    }
}

/// A registered environment, as an import leaves it before artwork. It
/// is made in a scratch root, so the page under test stays as staged.
fn registered() -> Arc<Registered> {
    let root = tempdir().unwrap();
    let mut source = grid_page(root.path());
    open_single(&mut source, DUSK);
    let listing = source.listing.clone().unwrap();
    let Review::Single { game, .. } =
        plan_review(&source.paths, &listing, Some(DUSK), &source.draft).unwrap()
    else {
        panic!("expected a single review");
    };
    let global = GlobalConfig::load(&source.paths).unwrap();
    Arc::new(
        apply_game(
            &source.paths,
            &global,
            &listing.source,
            &game,
            &Token::new(),
            |_| {},
        )
        .unwrap(),
    )
}

#[test]
fn short_dialogs_pin_every_steps_actions_below_the_scroll() {
    use crate::layout_probe::{bounds_at, probe, within};
    type Tweak = fn(&mut Page);
    const BUSY: &str = "Checking this game against the Heroic configuration...";
    // Each step in the states its footer can take: the plain pages from
    // `staged_pages`, then busy, retry, force stop, and registered ones.
    // The flag is whether the page has an action row to pin.
    let cases: [(&str, Tweak, bool); 12] = [
        ("single details", |_| {}, true),
        ("single review", |_| {}, true),
        ("batch details", |_| {}, true),
        ("batch review", |_| {}, true),
        ("working", |_| {}, true),
        ("finished", |_| {}, true),
        ("single details", |page| page.status = BUSY.to_owned(), true),
        ("batch details", |page| page.status = BUSY.to_owned(), true),
        ("single review", |page| page.status = BUSY.to_owned(), true),
        ("batch review", |page| page.status = BUSY.to_owned(), true),
        (
            "working",
            |page| {
                page.token.as_ref().unwrap().cancel();
                page.status = "Cancel requested...".to_owned();
            },
            true,
        ),
        ("working", |page| page.token = None, false),
    ];
    let artwork_retry: Tweak = |page| {
        page.registered = Some(registered());
        page.error = Some("Artwork failed: the server said no.".to_owned());
        page.token = None;
    };
    let registered_done: Tweak = |page| page.registered = Some(registered());
    let extra: [(&str, Tweak, bool); 2] = [
        ("working", artwork_retry, true),
        ("finished", registered_done, true),
    ];
    let root = tempdir().unwrap();
    for (width, height) in [(280.0, 240.0), (360.0, 200.0), (900.0, 240.0)] {
        let inset = iced::Rectangle::new(
            iced::Point::new(24.0, 24.0),
            iced::Size::new(width - 48.0, height - 48.0),
        );
        for (case, (label, tweak, pinned)) in cases.iter().chain(&extra).enumerate() {
            let mut pages = staged_pages(&root.path().join(format!("{case}-{width}")));
            let (_, page) = pages
                .iter_mut()
                .find(|(name, _)| name == label)
                .expect("staged page");
            tweak(page);
            let mut view = page.view_with_key_availability(Palette::DARK, width, false);
            let probe = probe(&mut view, iced::Size::new(width, height));
            let what = format!("{label} #{case} at {width}x{height}");
            assert_eq!(probe.scrollables.len(), 1, "{what}: one scrolling region");
            let (viewport, _) = probe.scrollables[0];
            assert!(viewport.height > 0.0, "{what}: {viewport:?}");
            let spilled = probe.outside(inset);
            assert!(spilled.is_empty(), "{what}: {spilled:?}");
            if !pinned {
                continue;
            }
            let actions = bounds_at(&probe.node, &[0, 1, 1]);
            assert!(within(actions, inset), "{what}: {actions:?}");
            assert!(
                actions.y >= viewport.y + viewport.height - 0.01,
                "{what}: {actions:?} under {viewport:?}"
            );
        }
    }
}

#[test]
fn keyboard_focus_scrolls_a_row_into_view_below_the_header() {
    use crate::layout_probe::{bounds_at, lay_out};
    let root = tempdir().unwrap();
    let page = grid_page(root.path());
    let rows = page.listing.as_ref().unwrap().candidates.len();
    // One column, so each game is its own row.
    for height in [700.0, 300.0] {
        let view = page.view_with_key_availability(Palette::DARK, 280.0, false);
        let mut laid = lay_out(view, iced::Size::new(280.0, height));
        let viewport = bounds_at(&laid.node, &[0, 1, 0]);
        for row in [rows - 1, 2, 0, 3, 1] {
            laid.run(Box::new(measure(GRID_ID, row, rows)));
            let offset = laid.offsets()[0];
            let tile = bounds_at(&laid.node, &[0, 1, 0, 0, 0, 1, 0, row]);
            let top = tile.y - offset;
            let bottom = tile.y + tile.height - offset;
            if tile.height <= viewport.height {
                assert!(
                    top >= viewport.y - 0.5 && bottom <= viewport.y + viewport.height + 0.5,
                    "row {row} at {height}: {tile:?} offset {offset} in {viewport:?}"
                );
            } else {
                assert!(
                    (top - viewport.y).abs() < 0.5,
                    "row {row} at {height}: {tile:?} offset {offset} in {viewport:?}"
                );
            }
            if row == 0 && tile.y - viewport.y + tile.height <= viewport.height {
                assert_eq!(offset, 0.0, "the header shows with the first row");
            }
        }
    }
}

#[test]
fn choosing_a_source_dismisses_the_discard_prompt() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    page.step = Step::Sources;
    page.sources = vec![HeroicSource::from_config_dir_with_home(
        root.path().join("heroic"),
        root.path().to_owned(),
    )];
    page.source = Some(0);
    let _ = page.update(Event::Back);
    assert!(page.leave_prompt);
    let _ = page.update(Event::Source(0));
    assert_eq!(page.step, Step::Games);
    assert!(!page.leave_prompt);
}

#[test]
fn several_sources_sit_inside_the_dialog_padding() {
    use crate::layout_probe::probe;
    let root = tempdir().unwrap();
    let mut page = grid_page(&root.path().join("sources"));
    page.step = Step::Sources;
    let long = format!(
        "/home/someone/.var/app/com.heroicgameslauncher.hgl/{}/heroic",
        "a-very-long-directory-name/".repeat(8)
    );
    page.sources = vec![
        HeroicSource::from_config_dir_with_home(root.path().join("heroic"), root.path().to_owned()),
        HeroicSource::from_config_dir_with_home(PathBuf::from(long), root.path().to_owned()),
    ];
    for (width, height) in [(280.0, 240.0), (360.0, 280.0), (900.0, 720.0)] {
        let inset = iced::Rectangle::new(
            iced::Point::new(24.0, 24.0),
            iced::Size::new(width - 48.0, height - 48.0),
        );
        let mut view = page.view_with_key_availability(Palette::DARK, width, false);
        let probe = probe(&mut view, iced::Size::new(width, height));
        assert_eq!(probe.scrollables.len(), 1, "one scrolling region");
        let spilled = probe.outside(inset);
        assert!(spilled.is_empty(), "{width}x{height}: {spilled:?}");
    }
}

#[test]
fn mouse_back_and_reopening_show_no_ring_until_a_key() {
    let root = tempdir().unwrap();
    let none = keyboard::Modifiers::empty();
    let mut page = grid_page(root.path());
    let _ = page.key(&Key::Named(Named::ArrowRight), none, 900.0);
    let focus = page.grid.focus.unwrap();
    assert!(page.grid.focus_ring(focus));
    // Space chooses the focused game, and Import opens its details.
    let _ = page.key(&Key::Named(Named::Space), none, 900.0);
    open_details(&mut page, focus);
    let _ = page.update(Event::Back);
    assert_eq!(page.step, Step::Games);
    assert!(!page.grid.focus_ring(focus));
    let _ = page.key(&Key::Named(Named::ArrowRight), none, 900.0);
    assert!(page.grid.focus_ring(page.grid.focus.unwrap()));

    // Leaving from the grid closes the page, and Add opens a fresh one.
    let mut flow = super::super::Page::Choose(page.paths.clone());
    let _ = flow.update(super::super::Event::Heroic, false);
    let super::super::Page::Heroic(reopened) = &mut flow else {
        panic!("expected the Heroic page");
    };
    reopened.step = Step::Games;
    reopened.listing = page.listing.clone();
    reopened.status.clear();
    let count = reopened.listing.as_ref().unwrap().candidates.len();
    assert!((0..count).all(|index| !reopened.grid.focus_ring(index)));
    let _ = flow.key(&Key::Named(Named::ArrowDown), none, 900.0);
    let super::super::Page::Heroic(reopened) = &flow else {
        unreachable!()
    };
    assert!(reopened.grid.focus_ring(reopened.grid.focus.unwrap()));
}

/// Prepares the chosen game the way the background task does.
fn open_details(page: &mut Page, index: usize) {
    let _ = page.update(Event::ImportSelected);
    let listing = page.listing.clone().unwrap();
    let prepared = prepare_game(
        &page.paths,
        &listing.source,
        listing.candidates[index].clone(),
    )
    .unwrap();
    let _ = page.update(Event::Prepared(page.generation, Ok(Box::new(prepared))));
    assert_eq!(page.step, Step::Details);
}

#[test]
fn toggling_and_select_all_skip_ineligible_games() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    assert!(
        page.ineligible(OLD)
            .unwrap()
            .contains("Already imported as old")
    );
    assert!(page.ineligible(STORE).is_some());
    let _ = page.update(Event::Toggle(OLD));
    let _ = page.update(Event::Toggle(STORE));
    assert!(page.grid.chosen.is_empty());
    let _ = page.update(Event::Toggle(DUSK));
    assert_eq!(chosen(&page), ["dusk"]);
    let _ = page.update(Event::Toggle(DUSK));
    assert!(page.grid.chosen.is_empty());
    let _ = page.update(Event::SelectAll);
    assert_eq!(chosen(&page), ["dawn", "dusk", "noon"]);
    let _ = page.update(Event::Toggle(DAWN));
    assert_eq!(chosen(&page), ["dusk", "noon"]);
    let _ = page.update(Event::SelectNone);
    assert!(page.grid.chosen.is_empty());
}

#[test]
fn importing_nothing_does_nothing() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    assert_eq!(import_label(0), "Import 0 games");
    assert_eq!(import_label(1), "Import 1 game");
    assert_eq!(import_label(3), "Import 3 games");
    let generation = page.generation;
    let _ = page.update(Event::ImportSelected);
    let _ = page.submit();
    assert_eq!(page.step, Step::Games);
    assert_eq!(page.generation, generation);
    assert_eq!(page.selected, None);
}

#[test]
fn one_chosen_game_opens_the_single_review() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    let _ = page.update(Event::Toggle(NOON));
    let _ = page.update(Event::ImportSelected);
    assert_eq!(page.selected, Some(NOON));
    let listing = page.listing.clone().unwrap();
    let prepared = prepare_game(
        &page.paths,
        &listing.source,
        listing.candidates[NOON].clone(),
    )
    .unwrap();
    let _ = page.update(Event::Prepared(page.generation, Ok(Box::new(prepared))));
    assert_eq!(page.step, Step::Details);
    assert!(!page.draft.batch);
    assert_eq!(page.draft.name, "noon");
    let _ = page.back();
    assert_eq!(page.step, Step::Games);
    assert_eq!(chosen(&page), ["noon"]);
}

#[test]
fn several_chosen_games_plan_a_batch_of_only_those() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    let _ = page.update(Event::Toggle(DUSK));
    let _ = page.update(Event::Toggle(NOON));
    let _ = page.update(Event::ImportSelected);
    assert_eq!(page.step, Step::Details);
    assert!(page.draft.batch);
    assert_eq!(
        page.draft.games,
        BTreeSet::from(["dusk".to_owned(), "noon".to_owned()])
    );
    let listing = page.listing.clone().unwrap();
    let Review::Batch(library) = plan_review(&page.paths, &listing, None, &page.draft).unwrap()
    else {
        panic!("expected a batch review");
    };
    let mut ready: Vec<&str> = library
        .entries()
        .iter()
        .map(|entry| match entry {
            Entry::Ready(game) => heroic_id(game),
            _ => panic!("expected only ready games"),
        })
        .collect();
    ready.sort();
    assert_eq!(ready, ["dusk", "noon"]);

    page.review = Some(Review::Batch(library));
    page.step = Step::Review;
    let _ = page.back();
    assert_eq!(page.step, Step::Details);
    let _ = page.back();
    assert_eq!(page.step, Step::Games);
    assert_eq!(chosen(&page), ["dusk", "noon"]);
}

#[test]
fn a_finished_plan_opens_review_with_a_scroll_to_the_top() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    open_single(&mut page, NOON);
    let task = page.plan();
    assert_eq!(page.step, Step::Details);
    drop(task);
    let listing = page.listing.clone().unwrap();
    let review = plan_review(&page.paths, &listing, page.selected, &page.draft).unwrap();
    let task = page.update(Event::Planned(page.generation, Ok(Box::new(review))));
    assert_eq!(page.step, Step::Review);
    assert!(page.status.is_empty());
    assert_eq!(task.units(), 1);

    let stale = page.update(Event::Planned(
        page.generation.wrapping_sub(1),
        Err("old".to_owned()),
    ));
    assert_eq!(stale.units(), 0);
}

#[test]
fn import_status_reads_as_a_sentence() {
    assert_eq!(
        import_status("Subverse", None, None),
        "Importing Subverse..."
    );
    assert_eq!(
        import_status(
            "Subverse",
            Some((2, 5)),
            Some("copying prefix (1,234 of 2,151 files)")
        ),
        "(2 of 5) Importing Subverse: copying prefix (1,234 of 2,151 files)"
    );
    assert_eq!(grouped(0), "0");
    assert_eq!(grouped(999), "999");
    assert_eq!(grouped(1000), "1,000");
    assert_eq!(grouped(1234567), "1,234,567");
}

#[test]
fn copy_progress_counts_files_and_throttles() {
    let start = Instant::now();
    let copied = || import::ImportProgress::Copied {
        path: PathBuf::from("drive_c"),
    };
    let mut copy = CopyProgress::default();
    assert_eq!(
        copy.record(&import::ImportProgress::Checking { entries: 12000 }, start)
            .as_deref(),
        Some("checking prefix (12,000 files)")
    );
    assert_eq!(
        copy.record(&import::ImportProgress::Started { entries: 2151 }, start)
            .as_deref(),
        Some("copying prefix (0 of 2,151 files)")
    );
    assert_eq!(copy.record(&copied(), start), None);
    for _ in 0..1232 {
        copy.record(&copied(), start + Duration::from_millis(50));
    }
    assert_eq!(
        copy.record(&copied(), start + PROGRESS_INTERVAL).as_deref(),
        Some("copying prefix (1,234 of 2,151 files)")
    );
    for _ in 0..916 {
        copy.record(&copied(), start + PROGRESS_INTERVAL);
    }
    // The last file shows even inside the interval.
    assert_eq!(
        copy.record(&copied(), start + PROGRESS_INTERVAL).as_deref(),
        Some("copying prefix (2,151 of 2,151 files)")
    );
    assert_eq!(
        copy.record(
            &import::ImportProgress::Finished { entries: 2151 },
            start + PROGRESS_INTERVAL
        )
        .as_deref(),
        Some("finishing")
    );
}

#[test]
fn shared_prefix_shows_only_when_used_in_place() {
    let shared = [
        HeroicSharedPrefix {
            id: "a".to_owned(),
            title: "Alpha".to_owned(),
        },
        HeroicSharedPrefix {
            id: "b".to_owned(),
            title: "Bravo".to_owned(),
        },
    ];
    assert_eq!(shared_prefix_note(false, &shared), None);
    assert_eq!(shared_prefix_note(true, &[]), None);
    assert_eq!(
        shared_prefix_note(true, &shared).as_deref(),
        Some("Shares prefix with: Alpha, Bravo")
    );
}

#[test]
fn skip_reasons_read_as_sentences() {
    assert_eq!(
        skip_reason(&SkipReason::NameTaken("dusk".to_owned())),
        "the name dusk is taken"
    );
    assert_eq!(
        skip_reason(&SkipReason::NeedsRunner { proton: true }),
        "needs a Proton runner"
    );
    assert_eq!(
        skip_reason(&SkipReason::Blocked("its prefix doesn't exist".to_owned())),
        "its prefix doesn't exist"
    );
}

#[test]
fn review_warnings_show_anti_cheat_once_and_nothing_else() {
    assert!(review_warnings(&[Warning::Omitted(3), Warning::Dropped(1)]).is_empty());
    assert_eq!(
        review_warnings(&[
            Warning::Omitted(2),
            Warning::AntiCheatNotImported,
            Warning::NeedsAttention(Vec::new()),
        ]),
        ["Heroic-managed anti-cheat runtime is not imported"]
    );
}

#[test]
fn hook_review_labels_marker_and_display_paths_are_safe() {
    let review = SettingsReview {
        carried: vec![
            wineo_core::heroic::ReviewItem {
                field: "before hook".to_owned(),
                detail: "/scripts/before\nreview 'game'.sh".to_owned(),
            },
            wineo_core::heroic::ReviewItem {
                field: "after hook".to_owned(),
                detail: "/scripts/after $game.sh".to_owned(),
            },
        ],
        ..SettingsReview::default()
    };
    assert_eq!(
        hook_paths(&review),
        [
            (
                "Before hook",
                "/scripts/before\\nreview 'game'.sh".to_owned()
            ),
            ("After hook", "/scripts/after $game.sh".to_owned()),
        ]
    );
    assert_eq!(hook_marker(&review), Some("runs launch scripts"));
    assert_eq!(
        wineo_core::heroic::display_control_escaped("/scripts/a\r\u{1b} b.sh"),
        "/scripts/a\\r\\u{1b} b.sh"
    );
}

#[test]
fn art_progress_counts_the_current_listings_posters_after_the_delay() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    let listing = page.listing.clone().unwrap();
    let ids: Vec<_> = listing
        .candidates
        .iter()
        .filter(|candidate| candidate.poster.is_some())
        .map(|candidate| candidate.id.clone())
        .collect();
    assert!(ids.len() >= 2);
    let total = ids.len();
    assert_eq!(
        page.grid.progress.pending_wakeup(),
        None,
        "idle owns no timer"
    );

    let _ = page.load_posters(&listing);
    let generation = page.grid.poster_generation;
    assert_eq!(page.grid.progress.shown(), None, "not before the delay");
    assert!(page.grid.progress.pending_wakeup().is_some());
    let _ = page.update(Event::ProgressDue(page.grid.progress.wakeup_generation()));
    assert_eq!(page.grid.progress.shown(), Some((0, total)));
    for width in [900.0, 360.0] {
        crate::layout_probe::layout(
            &mut page.view_with_key_availability(Palette::DARK, width, false),
            iced::Size::new(width, 600.0),
        );
    }

    // A failure counts like a success; a reply for an older listing counts
    // for nothing.
    let _ = page.update(Event::Poster(
        generation.wrapping_sub(1),
        ids[0].clone(),
        Err("late".into()),
    ));
    assert_eq!(page.grid.progress.shown(), Some((0, total)));
    let _ = page.update(Event::Poster(generation, ids[0].clone(), Err("bad".into())));
    assert_eq!(page.grid.progress.shown(), Some((1, total)));
    for id in &ids[1..] {
        let _ = page.update(Event::Poster(
            generation,
            id.clone(),
            Ok((
                image::Handle::from_rgba(1, 1, vec![1, 2, 3, 255]),
                Format::Png,
            )),
        ));
    }
    assert_eq!(page.grid.progress.shown(), None, "hidden at completion");
    assert_eq!(page.grid.progress.pending_wakeup(), None);
}

#[test]
fn art_progress_never_shows_when_the_posters_finish_before_the_delay() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    let listing = page.listing.clone().unwrap();
    let _ = page.load_posters(&listing);
    let generation = page.grid.poster_generation;
    let late = page.grid.progress.wakeup_generation();
    for candidate in &listing.candidates {
        if candidate.poster.is_some() {
            let _ = page.update(Event::Poster(
                generation,
                candidate.id.clone(),
                Err("x".into()),
            ));
        }
    }
    assert_eq!(page.grid.progress.pending_wakeup(), None);
    let _ = page.update(Event::ProgressDue(late));
    assert_eq!(page.grid.progress.shown(), None);
}

#[test]
fn a_new_listing_drops_the_old_listings_progress() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    let listing = page.listing.clone().unwrap();
    let _ = page.load_posters(&listing);
    let _ = page.update(Event::ProgressDue(page.grid.progress.wakeup_generation()));
    assert!(page.grid.progress.shown().is_some());
    let _ = page.list(listing.source.clone());
    assert_eq!(page.grid.progress.shown(), None);
    assert_eq!(page.grid.progress.pending_wakeup(), None);
}

/// Feeds the page each listed poster's decode, as the grid's workers would.
fn decode_posters(page: &mut Page) {
    let listing = page.listing.clone().unwrap();
    for candidate in &listing.candidates {
        if let Some(path) = &candidate.poster {
            let _ = page.update(Event::Poster(
                page.grid.poster_generation,
                candidate.id.clone(),
                artwork::preview_with_format(path, Kind::Grid),
            ));
        }
    }
}

/// Chooses only the game at `index` and opens its details.
fn open_single(page: &mut Page, index: usize) {
    let _ = page.update(Event::Toggle(index));
    let _ = page.update(Event::ImportSelected);
    let listing = page.listing.clone().unwrap();
    let prepared = prepare_game(
        &page.paths,
        &listing.source,
        listing.candidates[index].clone(),
    )
    .unwrap();
    let _ = page.update(Event::Prepared(page.generation, Ok(Box::new(prepared))));
    assert_eq!(page.step, Step::Details);
}

#[test]
fn missing_and_unreadable_posters_fall_back_to_the_title() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    let listing = page.listing.clone().unwrap();
    assert!(listing.candidates[NOON].poster.is_none());
    decode_posters(&mut page);
    assert!(page.poster("dusk").is_some());
    assert!(page.poster("dawn").is_none());
    assert!(page.poster("noon").is_none());
    let posters = page.batch_posters();
    assert_eq!(posters.keys().collect::<Vec<_>>(), ["dusk"]);
    assert!(posters["dusk"].decoded);
    // A reply for an earlier listing is dropped.
    let dusk = listing.candidates[DUSK].poster.clone().unwrap();
    let _ = page.update(Event::Poster(
        page.grid.poster_generation.wrapping_sub(1),
        "noon".to_owned(),
        artwork::preview_with_format(&dusk, Kind::Grid),
    ));
    assert!(page.poster("noon").is_none());
}

#[test]
fn inspecting_a_game_waits_by_the_import_button_until_its_reply() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    let listing = page.listing.clone().unwrap();
    let reply = Ok(Box::new(
        prepare_game(
            &page.paths,
            &listing.source,
            listing.candidates[NOON].clone(),
        )
        .unwrap(),
    ));
    assert!(!page.waiting());
    let _ = page.update(Event::Toggle(NOON));
    let _ = page.update(Event::ImportSelected);
    assert!(page.waiting());
    assert!(page.status_by_action());
    let flow = crate::flow::Page::Heroic(Box::new(page));
    assert!(flow.spinning());
    let crate::flow::Page::Heroic(page) = flow else {
        unreachable!()
    };
    let mut page = *page;
    let _ = page.update(Event::Prepared(page.generation, reply));
    assert!(!page.waiting());
}

#[test]
fn a_late_inspection_cannot_open_details_for_an_old_selection() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    let listing = page.listing.clone().unwrap();
    let paths = page.paths.clone();
    let prepared = || {
        Ok(Box::new(
            prepare_game(&paths, &listing.source, listing.candidates[NOON].clone()).unwrap(),
        ))
    };
    let reply = prepared();
    let _ = page.update(Event::Toggle(NOON));
    let _ = page.update(Event::ImportSelected);
    let pending = page.generation;
    let _ = page.update(Event::Toggle(DUSK));
    let _ = page.update(Event::Prepared(pending, reply));
    assert_eq!(page.step, Step::Games);
    assert!(page.prepared.is_none());
    assert!(page.status.is_empty());

    let reply = prepared();
    let _ = page.update(Event::SelectNone);
    let _ = page.update(Event::Toggle(NOON));
    let _ = page.update(Event::ImportSelected);
    let pending = page.generation;
    let _ = page.back();
    assert_eq!(page.step, Step::Sources);
    let _ = page.update(Event::Prepared(pending, reply));
    assert_eq!(page.step, Step::Sources);
    assert!(page.prepared.is_none());
}

#[test]
fn grid_keys_toggle_select_all_and_import() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    let none = keyboard::Modifiers::empty();
    let key = |page: &mut Page, key: Key, modifiers| page.key(&key, modifiers, 900.0);
    assert!(key(&mut page, Key::Named(Named::ArrowRight), none).is_some());
    assert_eq!(page.grid.focus, Some(DUSK));
    assert!(key(&mut page, Key::Named(Named::Space), none).is_some());
    assert_eq!(chosen(&page), ["dusk"]);
    let _ = key(&mut page, Key::Named(Named::ArrowRight), none);
    let _ = key(&mut page, Key::Named(Named::ArrowRight), none);
    let _ = key(&mut page, Key::Named(Named::ArrowRight), none);
    assert_eq!(page.grid.focus, Some(OLD));
    let _ = key(&mut page, Key::Named(Named::Space), none);
    assert_eq!(chosen(&page), ["dusk"]);
    let _ = key(
        &mut page,
        Key::Character("a".into()),
        keyboard::Modifiers::CTRL,
    );
    assert_eq!(chosen(&page), ["dawn", "dusk", "noon"]);
    // Other keys, like Escape for Back, are left to the flow.
    assert!(key(&mut page, Key::Named(Named::Escape), none).is_none());
    assert!(key(&mut page, Key::Named(Named::Enter), none).is_some());
    assert_eq!(page.step, Step::Details);
    assert!(page.draft.batch);
    assert!(key(&mut page, Key::Named(Named::Space), none).is_none());
}

#[test]
fn a_heroic_poster_is_prefilled_and_stored_as_grid_art() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    let heroic = root.path().join("heroic");
    let before = tree(&heroic);
    decode_posters(&mut page);
    open_single(&mut page, DUSK);
    assert_eq!(page.art.prefilled(Kind::Grid), Some("heroic-dusk.png"));
    let listing = page.listing.clone().unwrap();
    let Review::Single { game, .. } =
        plan_review(&page.paths, &listing, Some(DUSK), &page.draft).unwrap()
    else {
        panic!("expected a single review");
    };
    let paths = page.paths.clone();
    let global = GlobalConfig::load(&paths).unwrap();
    let registered = apply_game(
        &paths,
        &global,
        &listing.source,
        &game,
        &Token::new(),
        |_| {},
    )
    .unwrap();
    let name = registered.environment.name.as_str();
    page.art.apply(&paths, name).unwrap();
    let config = wineo_core::art::ArtConfig::load(&paths.art_dir(name)).unwrap();
    assert_eq!(
        config.kinds[&Kind::Grid].source,
        wineo_core::art::Source::Manual("heroic-dusk.png".to_owned())
    );
    assert_eq!(
        fs::read(paths.art_dir(name).join("grid.png")).unwrap(),
        fs::read(listing.candidates[DUSK].poster.as_ref().unwrap()).unwrap()
    );
    assert_eq!(tree(&heroic), before);
}

#[test]
fn a_corrupt_poster_is_neither_prefilled_nor_stored() {
    const FETCHED: &[u8] = b"\xff\xd8\xff\xe0 fetched";
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    decode_posters(&mut page);
    open_single(&mut page, DAWN);
    assert_eq!(page.art.prefilled(Kind::Grid), None);
    // A batch skips posters that failed to decode.
    assert!(!page.batch_posters().contains_key("dawn"));
    let paths = page.paths.clone();
    wineo_core::environment::Environment::create_native(
        &paths,
        &GlobalConfig::default(),
        "dawn",
        None,
        false,
        |_| {},
    )
    .unwrap();
    assert_eq!(store_heroic_poster(&paths, "dawn", "dawn", None), None);
    let art = artwork::Draft::with_fetched(
        Kind::Grid,
        7,
        wineo_core::art::Downloaded {
            asset: 42,
            bytes: FETCHED.to_vec(),
            format: wineo_core::art::Format::Jpeg,
        },
    );
    art.apply(&paths, "dawn").unwrap();
    let config = wineo_core::art::ArtConfig::load(&paths.art_dir("dawn")).unwrap();
    assert_eq!(
        config.kinds[&Kind::Grid].source,
        wineo_core::art::Source::SteamGridDb(42)
    );
}

#[test]
fn a_poster_that_decodes_after_details_open_is_prefilled_then() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    open_single(&mut page, DUSK);
    assert_eq!(page.art.prefilled(Kind::Grid), None);
    decode_posters(&mut page);
    assert_eq!(page.art.prefilled(Kind::Grid), Some("heroic-dusk.png"));
}

#[test]
fn a_removed_poster_is_not_prefilled_again() {
    let clear = |page: &mut Page| {
        let _ = page.update(Event::Art(
            page.art_generation,
            artwork::Event::Clear(Kind::Grid),
        ));
    };
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    decode_posters(&mut page);
    open_single(&mut page, DUSK);
    assert_eq!(page.art.prefilled(Kind::Grid), Some("heroic-dusk.png"));
    clear(&mut page);
    decode_posters(&mut page);
    assert_eq!(page.art.prefilled(Kind::Grid), None);

    // Removing it before the poster decodes counts too.
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    open_single(&mut page, DUSK);
    clear(&mut page);
    decode_posters(&mut page);
    assert_eq!(page.art.prefilled(Kind::Grid), None);
}

#[test]
fn a_batch_decodes_posters_the_grid_had_not_yet() {
    let root = tempdir().unwrap();
    let page = grid_page(root.path());
    let posters = page.batch_posters();
    assert!(!posters["dusk"].decoded);
    assert!(!posters["dawn"].decoded);
    let paths = page.paths.clone();
    for name in ["dusk", "dawn"] {
        wineo_core::environment::Environment::create_native(
            &paths,
            &GlobalConfig::default(),
            name,
            None,
            false,
            |_| {},
        )
        .unwrap();
        assert_eq!(
            store_heroic_poster(&paths, name, name, posters.get(name)),
            None
        );
    }
    let config = wineo_core::art::ArtConfig::load(&paths.art_dir("dusk")).unwrap();
    assert_eq!(
        config.kinds[&Kind::Grid].source,
        wineo_core::art::Source::Manual("heroic-dusk.png".to_owned())
    );
    assert!(
        !paths
            .art_dir("dawn")
            .join(wineo_core::art::ART_FILE)
            .exists()
    );
}

fn fake_proton(dir: &std::path::Path) {
    for program in ["proton", "files/bin/wine", "files/bin/wineserver"] {
        let file = dir.join(program);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        write_script(&file, "#!/bin/sh\nexit 0\n");
    }
}

/// Windows games Heroic runs on its own builds, in the config folder
/// wineo's scan looks at: Dusk on GE-Proton-latest, with its details
/// open, and Dawn on GE-Proton11-1. Noon is a native game.
fn windows_details(root: &std::path::Path) -> Page {
    let heroic = root.join(".config/heroic");
    for dir in ["sideload_apps", "GamesConfig", "store"] {
        fs::create_dir_all(heroic.join(dir)).unwrap();
    }
    let proton = heroic.join("tools/proton/GE-Proton-latest");
    fake_proton(&proton);
    let dawn_proton = heroic.join("tools/proton/GE-Proton11-1");
    fake_proton(&dawn_proton);
    let native = root.join("games/noon");
    fs::create_dir_all(native.parent().unwrap()).unwrap();
    write_script(&native, "#!/bin/sh\n");
    let prefix = root.join("prefix");
    fs::create_dir_all(prefix.join("pfx/drive_c")).unwrap();
    fs::write(
        prefix.join("pfx/system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    fs::write(
        heroic.join("config.json"),
        format!(
            r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"proton","name":"GE-Proton-latest","bin":"{}"}}}}}}"#,
            prefix.display(),
            proton.join("proton").display()
        ),
    )
    .unwrap();
    fs::write(
        heroic.join("sideload_apps/library.json"),
        format!(
            r#"{{"games":[{{"app_name":"dusk","title":"Dusk","runner":"sideload","install":{{"platform":"Windows","executable":"{exe}"}}}},{{"app_name":"dawn","title":"Dawn","runner":"sideload","install":{{"platform":"Windows","executable":"{exe}"}}}},{{"app_name":"noon","title":"Noon","runner":"sideload","install":{{"platform":"linux","executable":"{native}"}}}}]}}"#,
            exe = prefix.join("pfx/drive_c/game.exe").display(),
            native = native.display()
        ),
    )
    .unwrap();
    fs::write(heroic.join("GamesConfig/noon.json"), r#"{"noon":{}}"#).unwrap();
    fs::write(heroic.join("GamesConfig/dusk.json"), r#"{"dusk":{}}"#).unwrap();
    fs::write(
        heroic.join("GamesConfig/dawn.json"),
        format!(
            r#"{{"dawn":{{"wineVersion":{{"type":"proton","name":"GE-Proton11-1","bin":"{}"}}}}}}"#,
            dawn_proton.join("proton").display()
        ),
    )
    .unwrap();
    let paths = paths(root);
    let source = HeroicSource::from_config_dir_with_home(heroic, root.to_owned());
    let global = GlobalConfig::load(&paths).unwrap();
    let listing = heroic_import::listing(&paths, &global, &source).unwrap();
    assert_eq!(listing.candidates[0].kind, HeroicGameKind::Windows);
    let (mut page, _) = Page::new(paths);
    page.step = Step::Games;
    page.status.clear();
    page.listing = Some(listing);
    open_single(&mut page, 0);
    page
}

fn answer_single(page: &mut Page) {
    let _ = page.update(Event::Offer(true));
    assert!(page.answering);
    let listing = page.listing.clone().unwrap();
    let candidate = page.prepared.as_ref().unwrap().candidate.clone();
    let answered = runner_label::answer(&page.paths, true)
        .and_then(|()| refresh(&page.paths, Some((listing.source, candidate))));
    let _ = page.update(Event::Offered(
        page.offer_sequence,
        page.details_session,
        answered,
    ));
    assert!(!page.answering);
    assert!(!page.offer_shown());
}

#[test]
fn an_answer_for_one_game_does_not_change_the_next_games_runner() {
    let root = tempdir().unwrap();
    let mut page = windows_details(root.path());
    let dusk = page.prepared.as_ref().unwrap().candidate.clone();
    let _ = page.update(Event::Offer(true));
    let sequence = page.offer_sequence;
    let session = page.details_session;

    // Back to the grid and on to Dawn before Dusk's answer comes back.
    let _ = page.back();
    assert_eq!(page.step, Step::Games);
    let _ = page.update(Event::Toggle(0));
    open_single(&mut page, 1);
    assert_eq!(page.prepared.as_ref().unwrap().candidate.id, "dawn");
    assert!(page.draft.runner.is_empty());

    let listing = page.listing.clone().unwrap();
    let answered = runner_label::answer(&page.paths, true)
        .and_then(|()| refresh(&page.paths, Some((listing.source.clone(), dusk))));
    let _ = page.update(Event::Offered(sequence, session, answered));
    assert!(!page.answering);
    assert!(page.draft.runner.is_empty());
    assert!(!matches!(
        page.prepared.as_ref().unwrap().suggested,
        Some(RunnerSuggestion::Use(_))
    ));
    // Dawn's own choices are read instead.
    assert_ne!(page.offer_sequence, sequence);
    let dawn = page.prepared.as_ref().unwrap().candidate.clone();
    let relisted = refresh(&page.paths, Some((listing.source, dawn)));
    let _ = page.update(Event::Offered(
        page.offer_sequence,
        page.details_session,
        relisted,
    ));
    assert_eq!(page.draft.runner, "GE-Proton11-1");
    assert!(!page.offer_shown());
    assert_eq!(
        GlobalConfig::load(&page.paths).unwrap().external_runners,
        Some(true)
    );
}

#[test]
fn an_answer_landing_on_a_native_game_reads_no_runner_choices() {
    let root = tempdir().unwrap();
    let mut page = windows_details(root.path());
    let dusk = page.prepared.as_ref().unwrap().candidate.clone();
    let _ = page.update(Event::Offer(true));
    let sequence = page.offer_sequence;
    let session = page.details_session;

    let _ = page.back();
    let _ = page.update(Event::Toggle(0));
    open_single(&mut page, 2);
    assert_eq!(
        page.prepared.as_ref().unwrap().candidate.kind,
        HeroicGameKind::Native
    );

    let listing = page.listing.clone().unwrap();
    let answered = runner_label::answer(&page.paths, true)
        .and_then(|()| refresh(&page.paths, Some((listing.source, dusk))));
    let _ = page.update(Event::Offered(sequence, session, answered));
    // No replacement read starts, so nothing inspects Wine settings.
    assert_eq!(page.offer_sequence, sequence);
    assert!(!page.answering);
    assert!(page.error.is_none());
    assert!(!page.prepared.as_ref().unwrap().offer);
    assert_eq!(page.step, Step::Details);
}

#[test]
fn answering_yes_uses_heroics_build_in_place_for_a_single_game() {
    let root = tempdir().unwrap();
    let mut page = windows_details(root.path());
    assert!(page.offer_shown());
    assert!(matches!(
        page.prepared.as_ref().unwrap().suggested,
        Some(RunnerSuggestion::Copy(_))
    ));
    answer_single(&mut page);
    assert!(matches!(
        &page.prepared.as_ref().unwrap().suggested,
        Some(RunnerSuggestion::Use(name)) if name == "GE-Proton-latest"
    ));
    assert_eq!(page.draft.runner, "GE-Proton-latest");
    let listing = page.listing.clone().unwrap();
    let review = plan_review(&page.paths, &listing, page.selected, &page.draft).unwrap();
    let Review::Single { runner_line, .. } = review else {
        panic!("expected a single review");
    };
    assert_eq!(
        runner_line.as_deref(),
        Some("Runner: GE-Proton-latest (Heroic)")
    );
}

#[test]
fn answering_keeps_a_runner_the_person_chose_for_a_single_game() {
    let root = tempdir().unwrap();
    let mut page = windows_details(root.path());
    let _ = page.update(Event::Runner("my-proton".to_owned()));
    answer_single(&mut page);
    assert_eq!(page.draft.runner, "my-proton");
}

#[test]
fn answering_keeps_a_picked_batch_runner_even_when_it_was_the_default() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    fake_proton(&page.paths.default_runner_root().join("GE-Proton9-1"));
    crate::runner_label::fake_steam_build(root.path(), "GE-Proton11-3");
    let _ = page.update(Event::Toggle(DUSK));
    let _ = page.update(Event::Toggle(NOON));
    let _ = page.update(Event::ImportSelected);
    let _ = page.update(Event::BatchPrepared(
        page.generation,
        batch_options(&page.paths),
    ));
    assert_eq!(page.draft.runner, "GE-Proton9-1");
    let _ = page.update(Event::Runner("GE-Proton9-1".to_owned()));
    let _ = page.update(Event::Offer(true));
    let paths = page.paths.clone();
    let answered = runner_label::answer(&paths, true).and_then(|()| refresh(&paths, None));
    let Ok(Refreshed::Batch(options)) = &answered else {
        panic!("expected batch options");
    };
    assert_eq!(options.runner, "GE-Proton11-3");
    let _ = page.update(Event::Offered(
        page.offer_sequence,
        page.details_session,
        answered,
    ));
    assert_eq!(page.draft.runner, "GE-Proton9-1");
}

#[test]
fn a_late_answer_leaves_an_open_review_alone() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    let _ = page.update(Event::Toggle(DUSK));
    let _ = page.update(Event::Toggle(NOON));
    let _ = page.update(Event::ImportSelected);
    let _ = page.update(Event::BatchPrepared(
        page.generation,
        batch_options(&page.paths),
    ));
    let _ = page.update(Event::Offer(true));
    // The answer is still saving, so a second click does nothing.
    let sequence = page.offer_sequence;
    let _ = page.update(Event::Offer(false));
    assert_eq!(page.offer_sequence, sequence);

    let _ = page.update(Event::Plan);
    let listing = page.listing.clone().unwrap();
    let review = plan_review(&page.paths, &listing, None, &page.draft).map(Box::new);
    let _ = page.update(Event::Planned(page.generation, review));
    assert_eq!(page.step, Step::Review);
    let paths = page.paths.clone();
    let answered = runner_label::answer(&paths, true).and_then(|()| refresh(&paths, None));
    let _ = page.update(Event::Offered(sequence, page.details_session, answered));
    assert_eq!(page.step, Step::Review);
    assert!(page.review.is_some());
    assert!(!page.answering);
}

#[test]
fn the_runner_step_asks_once_and_answering_relists_in_place() {
    let root = tempdir().unwrap();
    let mut page = grid_page(root.path());
    crate::runner_label::fake_steam_build(root.path(), "GE-Proton11-3");
    let _ = page.update(Event::Toggle(DUSK));
    let _ = page.update(Event::Toggle(NOON));
    let _ = page.update(Event::ImportSelected);
    let _ = page.update(Event::BatchPrepared(
        page.generation,
        batch_options(&page.paths),
    ));
    assert!(page.offer_shown());
    assert!(page.batch_options.as_ref().unwrap().runners.is_empty());
    assert!(page.draft.runner.is_empty());

    let _ = page.update(Event::Offer(true));
    let paths = page.paths.clone();
    let answered = runner_label::answer(&paths, true).and_then(|()| refresh(&paths, None));
    let _ = page.update(Event::Offered(
        page.offer_sequence,
        page.details_session,
        answered,
    ));
    assert_eq!(
        GlobalConfig::load(&paths).unwrap().external_runners,
        Some(true)
    );
    assert!(!page.offer_shown());
    assert_eq!(page.step, Step::Details);
    assert_eq!(
        page.batch_options
            .as_ref()
            .unwrap()
            .runners
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["GE-Proton11-3 (Steam)"]
    );
    assert_eq!(page.draft.runner, "GE-Proton11-3");
}

#[test]
fn the_review_names_the_launcher_of_a_build_used_in_place() {
    let root = tempdir().unwrap();
    let paths = paths(root.path());
    crate::runner_label::fake_steam_build(root.path(), "GE-Proton11-3");
    wineo_core::settings::set_external_runners(&paths, true).unwrap();
    let global = GlobalConfig::load(&paths).unwrap();
    let runner = Runners::new(&paths, &global)
        .unwrap()
        .resolve("GE-Proton11-3")
        .unwrap();
    assert_eq!(
        runner_line(&paths, &runner),
        "Runner: GE-Proton11-3 (Steam)"
    );
}
