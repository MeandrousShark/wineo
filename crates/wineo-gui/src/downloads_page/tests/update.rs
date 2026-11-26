//! Update all: the check as a change, its dialog, and the batch.

use super::*;
use crate::pad::{PadEvent, PadId, PadInfo, Raw};

/// A gzip archive of one component build, laid out as upstream ships it.
fn archive(kind: Component, folder: &str) -> Vec<u8> {
    let marker = if kind == Component::Dxvk {
        "dxgi.dll"
    } else {
        "d3d12.dll"
    };
    let (wide, narrow) = kind.arch_dirs();
    let mut tar = tar::Builder::new(Vec::new());
    for arch in [wide, narrow] {
        let bytes = b"fake dll";
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, format!("{folder}/{arch}/{marker}"), &bytes[..])
            .unwrap();
    }
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gzip.write_all(&tar.into_inner().unwrap()).unwrap();
    gzip.finish().unwrap()
}

const DXVK: &str = "dxvk-2.7";
const VKD3D: &str = "vkd3d-proton-2.14";

/// DXVK 2.6 and vkd3d-proton 2.13 installed, with 2.7 and 2.14 upstream.
/// A family in `no_catalog` can't be asked about, and one in `no_archive`
/// can be checked but not downloaded.
fn two_updates(app: &App, no_catalog: &[Provider], no_archive: &[Provider]) -> Client<Fake> {
    component(&app.paths, Component::Dxvk, "dxvk-2.6");
    component(&app.paths, Component::Vkd3d, "vkd3d-proton-2.13");
    let mut urls = BTreeMap::new();
    for (provider, kind, tag, folder) in [
        (Provider::Dxvk, Component::Dxvk, "v2.7", DXVK),
        (Provider::Vkd3d, Component::Vkd3d, "v2.14", VKD3D),
    ] {
        let bytes = archive(kind, folder);
        let mut release = upstream(provider, tag);
        let asset = &mut release.assets[0];
        asset.name = format!("{folder}.tar.gz");
        asset.browser_download_url = format!("https://fake/{}", asset.name);
        asset.size = bytes.len() as u64;
        asset.digest = Some(format!("sha256:{:x}", Sha256::digest(&bytes)));
        if !no_archive.contains(&provider) {
            urls.insert(asset.browser_download_url.clone(), bytes);
        }
        if !no_catalog.contains(&provider) {
            urls.insert(catalog_url(provider, 1), page_bytes(&[release]));
        }
    }
    Client::with_transport(Fake(urls))
}

/// Clicks Update all and returns the check's generation and token.
fn begin_check(app: &mut App) -> (u64, Token) {
    send(Event::UpdateAll, app);
    let Some(removal::Change::Downloads(Change::Check(token))) = &app.change else {
        panic!("the check did not start")
    };
    (app.change_generation, token.clone())
}

/// Runs the check against `client` and hands its answer to the page.
fn check(app: &mut App, client: &Client<Fake>) -> Checked {
    let (generation, token) = begin_check(app);
    let checked = downloads_work::check_builds(client, &app.paths, &token);
    send(Event::Checked(generation, checked.clone()), app);
    checked
}

fn offers(app: &App) -> Vec<Offer> {
    match &app.downloads_page.review {
        Some(Review::Update(offers)) => offers.clone(),
        other => panic!("no update dialog: {other:?}"),
    }
}

fn names(offers: &[Offer]) -> Vec<(&str, &str)> {
    offers
        .iter()
        .map(|offer| {
            (
                offer.installed.as_str(),
                offer.release.installed_name.as_str(),
            )
        })
        .collect()
}

/// Confirms the open dialog and returns the batch's generation and token.
fn begin_batch(app: &mut App) -> (u64, Token) {
    send(Event::Confirm, app);
    let Some(removal::Change::Downloads(Change::Update { token, .. })) = &app.change else {
        panic!("the batch did not start")
    };
    (app.change_generation, token.clone())
}

fn reason(app: &App) -> String {
    app.change.as_ref().unwrap().reason()
}

fn dialog_texts(app: &App) -> Vec<String> {
    crate::layout_probe::texts(app.downloads_confirm_view(), app.window_size)
}

#[test]
fn update_all_checks_as_a_change_that_holds_other_actions_until_it_answers() {
    let (_root, mut app) = fixture();
    let client = two_updates(&app, &[], &[]);
    open(&mut app);
    let old = build(&app, Kind::Dxvk, "dxvk-2.6");
    let (generation, token) = begin_check(&mut app);
    assert_eq!(
        app.downloads_page.status.as_deref(),
        Some("Checking for newer builds")
    );
    assert_eq!(reason(&app), "Busy: checking for newer builds");
    assert!(app.management_busy());
    assert!(app.spinning() && app.foreground_busy());

    let page_generation = app.downloads_page.generation;
    send(Event::Refresh, &mut app);
    assert_eq!(app.downloads_page.generation, page_generation);
    send(Event::Choose, &mut app);
    assert!(!app.downloads_page.choosing);
    send(Event::Review(old.id()), &mut app);
    assert!(app.downloads_page.review.is_none());
    send(Event::UpdateAll, &mut app);
    assert_eq!(app.change_generation, generation);

    let checked = downloads_work::check_builds(&client, &app.paths, &token);
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    send(
        Event::Checked(generation.wrapping_sub(1), checked.clone()),
        &mut app,
    );
    assert!(app.change.is_some());
    send(Event::Checked(generation, checked), &mut app);
    assert!(app.change.is_none());
    assert!(app.downloads_page.status.is_none());
    assert!(app.downloads_confirm_open());
    assert_eq!(
        names(&offers(&app)),
        [("dxvk-2.6", DXVK), ("vkd3d-proton-2.13", VKD3D)]
    );
    assert_eq!(
        app.downloads_confirm_controls(),
        vec![CANCEL_ID, CONFIRM_ID]
    );
    let texts = dialog_texts(&app);
    for line in [
        "Update downloads",
        "Download these 2 newer builds?",
        "DXVK, replacing dxvk-2.6",
        "vkd3d-proton, replacing vkd3d-proton-2.13",
        "Older builds are kept.",
        "Download",
    ] {
        assert!(texts.iter().any(|text| text == line), "{line}: {texts:?}");
    }
    assert!(
        texts
            .iter()
            .any(|text| text.starts_with("dxvk-2.7 (") && text.ends_with(" B)")),
        "{texts:?}"
    );
    // Dismissing leaves everything as it was.
    send(Event::Dismiss, &mut app);
    assert!(!app.downloads_confirm_open());
    assert!(app.change.is_none());
}

#[test]
fn cancel_stops_the_check_and_says_so() {
    let (_root, mut app) = fixture();
    let client = two_updates(&app, &[], &[]);
    open(&mut app);
    let (generation, token) = begin_check(&mut app);
    send(Event::Cancel, &mut app);
    assert!(token.is_canceled());
    assert_eq!(
        app.downloads_page.status.as_deref(),
        Some("Canceling check...")
    );
    let checked = downloads_work::check_builds(&client, &app.paths, &token);
    assert!(checked.canceled);
    // The families the cancel skipped aren't errors to show.
    assert!(checked.errors.is_empty(), "{:?}", checked.errors);
    send(Event::Checked(generation, checked), &mut app);
    assert!(app.change.is_none());
    assert!(app.downloads_page.review.is_none());
    assert!(app.downloads_page.error.is_none());
    assert_eq!(
        app.downloads_page.notice.as_deref(),
        Some("Check canceled.")
    );
}

#[test]
fn up_to_date_and_nothing_installed_each_say_so() {
    let (_root, mut app) = fixture();
    open(&mut app);
    // Nothing installed asks nothing, so an empty catalog is enough.
    let checked = check(&mut app, &Client::with_transport(Fake(BTreeMap::new())));
    assert_eq!(checked.families, 0);
    assert!(app.change.is_none());
    assert!(app.downloads_page.review.is_none());
    assert!(app.downloads_page.error.is_none());
    assert_eq!(
        app.downloads_page.notice.as_deref(),
        Some("Nothing to update.")
    );

    let client = two_updates(&app, &[], &[]);
    component(&app.paths, Component::Dxvk, DXVK);
    component(&app.paths, Component::Vkd3d, VKD3D);
    load(&mut app);
    check(&mut app, &client);
    assert!(app.downloads_page.review.is_none());
    assert!(app.downloads_page.error.is_none());
    assert_eq!(
        app.downloads_page.notice.as_deref(),
        Some("Everything is up to date.")
    );
}

#[test]
fn two_failed_checks_both_show_and_one_failure_leaves_the_others_offer() {
    let (_root, mut app) = fixture();
    let client = two_updates(&app, &[Provider::Dxvk, Provider::Vkd3d], &[]);
    open(&mut app);
    check(&mut app, &client);
    assert!(app.downloads_page.review.is_none());
    assert!(app.downloads_page.notice.is_none());
    let error = app.downloads_page.error.clone().unwrap();
    let lines: Vec<&str> = error.lines().collect();
    assert_eq!(lines.len(), 2, "{error}");
    assert!(lines[0].starts_with("Couldn't check DXVK: "), "{error}");
    assert!(
        lines[1].starts_with("Couldn't check vkd3d-proton: "),
        "{error}"
    );
    let _ = app.downloads_view();

    let (_root, mut app) = fixture();
    let client = two_updates(&app, &[Provider::Vkd3d], &[]);
    open(&mut app);
    check(&mut app, &client);
    assert_eq!(names(&offers(&app)), [("dxvk-2.6", DXVK)]);
    assert!(
        app.downloads_page
            .error
            .as_deref()
            .unwrap()
            .starts_with("Couldn't check vkd3d-proton: ")
    );
    assert!(dialog_texts(&app).contains(&"Download this newer build?".to_owned()));
}

#[test]
fn keys_and_the_controller_go_to_the_update_dialog() {
    let (_root, mut app) = fixture();
    let client = two_updates(&app, &[], &[]);
    open(&mut app);
    let _ = app.update(Message::Pad(PadEvent::Connected(
        PadId::Gilrs(0),
        PadInfo {
            name: "Xbox Wireless Controller".to_owned(),
            vendor: Some(0x045e),
            product: Some(0x0b13),
        },
    )));
    let id = app.main_window.expect("the main window");
    let _ = app.update(Message::Window(id, iced::window::Event::Focused));
    for close in ["escape", "pad"] {
        check(&mut app, &client);
        assert!(app.downloads_confirm_open());
        assert!(app.dialog_open());
        crate::shortcuts::tests::press(&mut app, "1", Modifiers::CTRL);
        assert_eq!(app.section, sidebar::Section::Downloads);
        let _ = focus_button::take_cycles();
        let _ = app.update(Message::Key(
            Key::Named(Named::Tab),
            Modifiers::empty(),
            Status::Ignored,
        ));
        assert_eq!(focus_button::take_cycles().len(), 1);
        // A stands for Enter, which only a focused button takes here.
        let _ = app.update(Message::Pad(PadEvent::Pressed(PadId::Gilrs(0), Raw::South)));
        assert!(app.downloads_confirm_open());
        assert!(app.change.is_none());
        if close == "escape" {
            let _ = app.update(Message::Key(
                Key::Named(Named::Escape),
                Modifiers::empty(),
                Status::Ignored,
            ));
        } else {
            let _ = app.update(Message::Pad(PadEvent::Pressed(PadId::Gilrs(0), Raw::East)));
        }
        assert!(!app.downloads_confirm_open(), "{close}");
        assert!(app.change.is_none());
    }
}

#[test]
fn the_batch_stays_busy_between_downloads_and_reports_both() {
    let (_root, mut app) = fixture();
    let client = two_updates(&app, &[], &[]);
    open(&mut app);
    check(&mut app, &client);
    let offers = offers(&app);
    let (generation, token) = begin_batch(&mut app);
    assert!(!app.downloads_confirm_open());
    assert_eq!(
        app.downloads_page.status.as_deref(),
        Some("Starting download...")
    );
    assert_eq!(reason(&app), format!("Busy: downloading {DXVK}"));
    let paths = app.paths.clone();
    let mut lines = Vec::new();
    let mut between = false;
    let applied = downloads_work::update_builds(&client, &paths, &offers, &token, |name, line| {
        if name == VKD3D && !between {
            // The first build is in and the second has begun: still busy.
            between = true;
            assert!(app.change.is_some() && app.management_busy());
        }
        lines.push(line.clone());
        send(Event::Stepped(generation, name, line), &mut app);
        assert_eq!(app.downloads_page.status.as_ref(), lines.last());
    });
    assert!(between);
    assert_eq!(reason(&app), format!("Busy: downloading {VKD3D}"));
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("1 of 2: Downloaded ") && line.contains(" of ")),
        "{lines:?}"
    );
    assert!(
        lines.contains(&"2 of 2: Extracting vkd3d".to_owned()),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .all(|line| line.starts_with("1 of 2: ") || line.starts_with("2 of 2: ")),
        "{lines:?}"
    );
    assert_eq!(applied.installed, [DXVK, VKD3D]);
    assert!(applied.errors.is_empty(), "{:?}", applied.errors);
    send(
        Event::Updated(generation.wrapping_sub(1), applied.clone()),
        &mut app,
    );
    assert!(app.change.is_some());
    let page_generation = app.downloads_page.generation;
    send(Event::Updated(generation, applied), &mut app);
    assert!(app.change.is_none());
    assert!(app.downloads_page.status.is_none());
    assert_eq!(
        app.downloads_page.generation,
        page_generation.wrapping_add(1)
    );
    assert_eq!(
        app.downloads_page.notice.as_deref(),
        Some("Installed dxvk-2.7 and vkd3d-proton-2.14.")
    );
    assert!(app.downloads_page.error.is_none());
    // Older builds stay beside the new ones.
    load(&mut app);
    for (kind, name) in [
        (Kind::Dxvk, "dxvk-2.6"),
        (Kind::Dxvk, DXVK),
        (Kind::Vkd3d, "vkd3d-proton-2.13"),
        (Kind::Vkd3d, VKD3D),
    ] {
        assert_eq!(build(&app, kind, name).name, name);
    }
}

#[test]
fn a_failed_download_is_listed_and_the_rest_still_install() {
    let (_root, mut app) = fixture();
    let client = two_updates(&app, &[], &[Provider::Dxvk]);
    open(&mut app);
    check(&mut app, &client);
    let offers = offers(&app);
    let (generation, token) = begin_batch(&mut app);
    let applied = downloads_work::update_builds(&client, &app.paths, &offers, &token, |_, _| {});
    assert_eq!(applied.installed, [VKD3D]);
    send(Event::Updated(generation, applied), &mut app);
    assert_eq!(
        app.downloads_page.notice.as_deref(),
        Some("Installed vkd3d-proton-2.14.")
    );
    let error = app.downloads_page.error.clone().unwrap();
    assert!(error.starts_with("Couldn't download dxvk-2.7: "), "{error}");
    assert_eq!(error.lines().count(), 1, "{error}");
    assert!(!app.paths.default_dxvk_root().join(DXVK).exists());
    assert!(app.paths.default_vkd3d_root().join(VKD3D).is_dir());
    let _ = app.downloads_view();
}

#[test]
fn cancel_during_the_batch_keeps_finished_builds_and_skips_the_rest() {
    let (_root, mut app) = fixture();
    let client = two_updates(&app, &[], &[]);
    open(&mut app);
    check(&mut app, &client);
    let offers = offers(&app);
    let (generation, token) = begin_batch(&mut app);
    let paths = app.paths.clone();
    let applied = downloads_work::update_builds(&client, &paths, &offers, &token, |name, line| {
        let published = line.starts_with("1 of 2: Installed in");
        send(Event::Stepped(generation, name, line), &mut app);
        if published {
            send(Event::Cancel, &mut app);
        }
    });
    assert!(token.is_canceled());
    assert_eq!(
        app.downloads_page.status.as_deref(),
        Some("Canceling download...")
    );
    // A step that arrives after Cancel doesn't hide that it's stopping.
    send(
        Event::Stepped(generation, VKD3D.into(), "2 of 2: Verifying vkd3d".into()),
        &mut app,
    );
    assert_eq!(
        app.downloads_page.status.as_deref(),
        Some("Canceling download...")
    );
    assert_eq!(applied.installed, [DXVK]);
    assert!(applied.canceled);
    assert!(applied.errors.is_empty(), "{:?}", applied.errors);
    send(Event::Updated(generation, applied), &mut app);
    assert!(app.change.is_none());
    assert_eq!(
        app.downloads_page.notice.as_deref(),
        Some("Installed dxvk-2.7. Update canceled.")
    );
    assert!(app.downloads_page.error.is_none());
    assert!(paths.default_dxvk_root().join(DXVK).is_dir());
    assert!(!paths.default_vkd3d_root().join(VKD3D).exists());
}

#[test]
fn closing_the_window_stops_the_check_and_the_batch() {
    for batch in [false, true] {
        let (_root, mut app) = fixture();
        let client = two_updates(&app, &[], &[]);
        open(&mut app);
        let (generation, token) = if batch {
            check(&mut app, &client);
            begin_batch(&mut app)
        } else {
            begin_check(&mut app)
        };
        let _ = app.update(Message::Section(sidebar::Section::Library));
        assert!(!token.is_canceled());
        let _ = app.update(app.window_event(iced::window::Event::CloseRequested));
        assert!(app.close_prompt && !app.closing);
        let _ = app.update(Message::CancelAndClose);
        assert!(token.is_canceled(), "batch {batch}");
        assert!(app.closing && app.foreground_busy());
        if batch {
            send(Event::Updated(generation, Applied::default()), &mut app);
        } else {
            send(Event::Checked(generation, Checked::default()), &mut app);
        }
        assert!(app.change.is_none());
        assert!(!app.foreground_busy());
    }
}

#[test]
fn a_check_answered_after_leaving_the_page_is_dropped() {
    let (_root, mut app) = fixture();
    let client = two_updates(&app, &[], &[]);
    open(&mut app);
    let (generation, token) = begin_check(&mut app);
    let _ = app.update(Message::Section(sidebar::Section::Library));
    assert!(!token.is_canceled());
    let checked = downloads_work::check_builds(&client, &app.paths, &token);
    assert_eq!(checked.offers.len(), 2);
    send(Event::Checked(generation, checked), &mut app);
    assert!(app.change.is_none());
    assert!(app.downloads_page.review.is_none());
    assert!(app.downloads_page.notice.is_none());
    let _ = app.update(Message::Section(sidebar::Section::Downloads));
    assert!(!app.downloads_confirm_open());
}

#[test]
fn the_update_list_scrolls_so_the_buttons_stay_in_a_short_window() {
    let (_root, mut app) = fixture();
    open(&mut app);
    app.window_size = iced::Size::new(400.0, 300.0);
    let offers = (1..=20)
        .map(|minor| Offer {
            installed: "dxvk-1.0".into(),
            release: select_asset(
                Provider::Dxvk,
                upstream(Provider::Dxvk, &format!("v2.{minor}")),
            )
            .unwrap()
            .unwrap(),
        })
        .collect();
    app.downloads_page.review = Some(Review::Update(offers));
    let mut view: Element<'_, Message> =
        container(app.downloads_confirm_view()).center(Fill).into();
    let probe = crate::layout_probe::probe(&mut view, app.window_size);
    let frame = iced::Rectangle::with_size(app.window_size);
    let spilled = probe.outside(frame);
    assert!(spilled.is_empty(), "{spilled:?}");
    let [(viewport, content)] = probe.scrollables.as_slice() else {
        panic!("one scrolling list: {:?}", probe.scrollables)
    };
    assert_eq!(viewport.height, app.update_list_height());
    assert!(content.height > viewport.height);
}

/// The header's three buttons, by their bounds: the page's scrollable, its
/// padded column, the header, then the right group.
fn header_buttons(app: &App, width: f32) -> Vec<iced::Rectangle> {
    let size = iced::Size::new(width, 300.0);
    let mut view = app.downloads_view();
    let node = crate::layout_probe::layout(&mut view, size);
    (0..3)
        .map(|index| crate::layout_probe::bounds_at(&node, &[0, 0, 0, 1, index]))
        .collect()
}

#[test]
fn the_header_fits_three_buttons_at_400_px() {
    let (_root, mut app) = fixture();
    open(&mut app);
    app.window_size = iced::Size::new(400.0, 300.0);
    let width = 400.0 - sidebar::WIDTH;
    let natural = header_buttons(&app, 4000.0);
    let narrow = header_buttons(&app, width);
    assert_eq!(narrow.len(), 3);
    for (button, natural) in narrow.iter().zip(&natural) {
        assert_eq!(button.width, natural.width, "a button lost its width");
        assert!(button.x + button.width <= width - 24.0, "{button:?}");
    }
    // They wrap onto a second line rather than squeeze.
    assert!(narrow[2].y > narrow[0].y);
    let size = iced::Size::new(width, 300.0);
    let mut view = app.downloads_view();
    let spilled =
        crate::layout_probe::probe(&mut view, size).outside(iced::Rectangle::with_size(size));
    assert!(spilled.is_empty(), "{spilled:?}");
    let texts = crate::layout_probe::texts(app.downloads_view(), size);
    for label in ["Refresh", "Update all", "Download..."] {
        assert!(texts.iter().any(|text| text == label), "{label}: {texts:?}");
    }
}
