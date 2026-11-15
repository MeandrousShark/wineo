//! Import prefix's art: the fetch checkbox, the fetch after the import, and
//! what Working, ArtFailed, and Finished do around it.

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::test_script::write_script;
use wineo_core::environment::Environment;

use super::*;

fn paths(root: &Path) -> Paths {
    let home = root.to_owned();
    Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
        .unwrap()
}

fn fake_runner(paths: &Paths) {
    let root = paths.default_runner_root().join("fake-wine").join("bin");
    fs::create_dir_all(&root).unwrap();
    for program in ["wine", "wineserver"] {
        write_script(root.join(program), "#!/bin/sh\nexit 0\n");
    }
}

fn source_prefix(root: &Path) -> PathBuf {
    let prefix = root.join("source-prefix");
    fs::create_dir_all(prefix.join("drive_c")).unwrap();
    fs::write(
        prefix.join("system.reg"),
        "WINE REGISTRY Version 2\n#arch=win64\n",
    )
    .unwrap();
    prefix
}

/// What a fake fetcher saw on a call.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Call {
    name: String,
    title: String,
    /// Whether the environment was registered when the fetch ran.
    registered: bool,
    /// What `busy_with` said about the environment's lock.
    lock: String,
    canceled: bool,
}

type Calls = Arc<std::sync::Mutex<Vec<Call>>>;

/// A fetcher that records each call and answers `answer`, or an error once
/// its token is canceled. It writes a file only when it isn't canceled, as
/// the real fetch writes art.
fn fake_fetcher(answer: Result<Vec<String>, String>) -> (Fetcher, Calls) {
    let calls = Calls::default();
    let seen = calls.clone();
    let fetcher: Fetcher = Arc::new(move |paths, name, title, token| {
        let lock = format!(
            "{:?}",
            wineo_core::operation::Locks::new(paths).busy_with(name)
        );
        seen.lock().unwrap().push(Call {
            name: name.to_owned(),
            title: title.to_owned(),
            registered: paths.environment_config(name).exists(),
            lock,
            canceled: token.is_canceled(),
        });
        if token.is_canceled() {
            return Err("canceled".to_owned());
        }
        fs::write(paths.applications_dir().join("fetched.marker"), "").ok();
        answer.clone()
    });
    (fetcher, calls)
}

fn call_count(calls: &Calls) -> usize {
    calls.lock().unwrap().len()
}

/// Runs `task` to its end and returns the events it produced, giving up
/// after a few seconds so a stuck worker fails the test instead of hanging.
fn outputs(task: Task<Event>) -> Vec<Event> {
    use iced::futures::StreamExt;
    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return Vec::new();
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut events = Vec::new();
        while let Ok(Some(action)) =
            tokio::time::timeout(Duration::from_secs(20), stream.next()).await
        {
            if let iced_runtime::Action::Output(event) = action {
                events.push(event);
            }
        }
        events
    })
}

/// Feeds `task`'s events back into `page`, and theirs, until nothing is left.
fn drive(page: &mut Page, task: Task<Event>, key_available: bool) {
    let mut pending = vec![task];
    for _ in 0..64 {
        let Some(task) = pending.pop() else {
            return;
        };
        for event in outputs(task) {
            pending.push(page.update_with_key_availability(event, key_available));
        }
    }
    panic!("the page kept starting work");
}

/// A page reviewed and ready to import, with the settings read as if a key
/// were set and Art to fetch had a kind on.
fn reviewed(root: &Path) -> (Page, Calls) {
    let paths = paths(root);
    fake_runner(&paths);
    let source = source_prefix(root);
    let mut page = Page::new(paths).0;
    page.source = source.display().to_string();
    page.name = "imported".to_owned();
    page.runner = "fake-wine".to_owned();
    page.fetch_kinds = Some(true);
    page.review = Some(
        inspect_review(
            &page.paths,
            &page.source,
            &page.name,
            &page.runner,
            false,
            "",
            None,
        )
        .unwrap(),
    );
    page.step = Step::Review;
    let (fetcher, calls) = fake_fetcher(Ok(vec!["No logo artwork found".to_owned()]));
    page.fetcher = fetcher;
    (page, calls)
}

/// A page on Working, fetching art for an environment it imported.
fn fetching(root: &Path) -> (Page, Calls, Task<Event>) {
    let (mut page, calls) = reviewed(root);
    page.imported_name = Some("imported".to_owned());
    page.fetch_art = true;
    let task = page.start_fetch();
    (page, calls, task)
}

#[test]
fn the_fetch_runs_after_the_import_and_never_inside_its_worker() {
    let root = tempfile::tempdir().unwrap();
    let (mut page, calls) = reviewed(root.path());
    let task = page.update_with_key_availability(Event::Import, true);
    assert!(page.fetch_art);
    // The import's own work ends without a fetch.
    let events = outputs(task);
    assert_eq!(call_count(&calls), 0);
    let finished = events
        .into_iter()
        .find(|event| matches!(event, Event::ImportFinished(..)))
        .expect("the import finished");
    assert!(page.paths.environment_config("imported").exists());

    let task = page.update_with_key_availability(finished, true);
    assert_eq!(page.step, Step::Working);
    assert!(page.fetching);
    assert_eq!(page.status, "Fetching artwork...");
    assert_eq!(call_count(&calls), 0, "the fetch is its own task");
    drive(&mut page, task, true);

    assert_eq!(
        *calls.lock().unwrap(),
        [Call {
            name: "imported".to_owned(),
            title: "imported".to_owned(),
            registered: true,
            lock: "Ok(None)".to_owned(),
            canceled: false,
        }]
    );
    assert_eq!(page.step, Step::Finished);
    assert_eq!(page.notes, ["No logo artwork found"]);
    assert!(page.error.is_none());
}

/// Puts a desktop entry on disk for `name`, then drops the environment that
/// asked for it, so only a later `desktop::apply` for `name` removes it.
fn stale_entry(paths: &Paths, root: &Path, name: &str) -> PathBuf {
    let exe = root.join("game.sh");
    write_script(&exe, "#!/bin/sh\n");
    Environment::create_native_titled(
        paths,
        &GlobalConfig::default(),
        name,
        Some(&exe.display().to_string()),
        Some("Game"),
        true,
        |_| {},
    )
    .unwrap();
    desktop::apply(paths, name, false).unwrap();
    let entries: Vec<_> = fs::read_dir(paths.applications_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
    fs::remove_file(paths.environment_config(name)).unwrap();
    entries[0].clone()
}

#[test]
fn the_entry_is_refreshed_even_with_the_checkbox_off() {
    let root = tempfile::tempdir().unwrap();
    let (mut page, calls) = reviewed(root.path());
    let stale = stale_entry(&page.paths, root.path(), "imported");
    // The checkbox lives on Details.
    page.step = Step::Details;
    let _ = page.update_with_key_availability(Event::FetchArt(false), true);
    page.step = Step::Review;
    let task = page.update_with_key_availability(Event::Import, true);
    assert!(!page.fetch_art);
    drive(&mut page, task, true);
    assert_eq!(page.step, Step::Finished, "{:?}", page.error);
    assert_eq!(call_count(&calls), 0);
    assert!(!stale.exists(), "the worker applied the desktop entry");
}

#[test]
fn skip_back_and_esc_cancel_the_fetch_and_land_on_finished() {
    for how in ["Skip", "Back", "Esc"] {
        let root = tempfile::tempdir().unwrap();
        let (mut page, calls, task) = fetching(root.path());
        assert_eq!(page.step, Step::Working, "{how}");
        assert!(!page.busy(), "{how}: fetching doesn't hold the window");
        let token = page.fetch_token.clone().unwrap();
        let _ = match how {
            "Skip" => page.update_with_key_availability(Event::SkipFetch, true),
            "Back" => page.back(),
            // Esc through the app's key path, which sends the flow Back.
            _ => {
                let mut app = crate::tests::app();
                app.flow = Some(crate::flow::Page::Prefix(Box::new(page)));
                let _ = app.update(crate::Message::Key(
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                    iced::keyboard::Modifiers::default(),
                    iced::event::Status::Ignored,
                ));
                let Some(crate::flow::Page::Prefix(open)) = app.flow.take() else {
                    panic!("not on the Import prefix page");
                };
                page = *open;
                Task::none()
            }
        };
        assert!(token.is_canceled(), "{how}");
        assert_eq!(page.status, "Skipping artwork...", "{how}");
        assert_eq!(page.step, Step::Working, "{how}: the worker lands it");
        drive(&mut page, task, true);
        assert_eq!(page.step, Step::Finished, "{how}");
        assert!(page.error.is_none(), "{how}");
        assert_eq!(page.notes, ["Artwork skipped"], "{how}");
        // The fetch saw the token already canceled, so it wrote nothing.
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "{how}");
        assert!(calls[0].canceled, "{how}");
        assert!(
            !page
                .paths
                .applications_dir()
                .join("fetched.marker")
                .exists(),
            "{how}: nothing is written after a Skip"
        );
    }
}

#[test]
fn back_does_nothing_on_working_when_nothing_fetches() {
    let root = tempfile::tempdir().unwrap();
    let (mut page, _) = reviewed(root.path());
    page.step = Step::Working;
    page.token = Some(Token::new());
    let _ = page.back();
    assert_eq!(page.step, Step::Working);
    assert!(!page.token.as_ref().unwrap().is_canceled());
    assert!(page.busy());
}

#[test]
fn a_late_warning_still_shows_and_a_late_progress_does_not_overwrite_the_fetch() {
    let root = tempfile::tempdir().unwrap();
    let (mut page, _calls) = reviewed(root.path());
    page.imported_name = Some("imported".to_owned());
    // The import's stream carries the generation it started under.
    let generation = page.generation;
    let _task = page.start_fetch();
    assert!(page.fetching);
    let _ = page.update_with_key_availability(
        Event::Warning(generation, "a file could not be copied".to_owned()),
        true,
    );
    assert_eq!(page.warning.as_deref(), Some("a file could not be copied"));
    let _ = page.update_with_key_availability(
        Event::Progress(generation, "Copied 250 files...".to_owned()),
        true,
    );
    assert_eq!(page.status, "Fetching artwork...");

    // Working without a fetch still takes progress.
    page.fetching = false;
    let _ = page.update_with_key_availability(
        Event::Progress(generation, "Copied 500 files...".to_owned()),
        true,
    );
    assert_eq!(page.status, "Copied 500 files...");
}

#[test]
fn the_fetch_searches_the_name_as_typed_for_the_imported_environment() {
    let root = tempfile::tempdir().unwrap();
    let (mut page, calls) = reviewed(root.path());
    page.name = "Hollow Knight".to_owned();
    page.fetch_art = true;
    page.step = Step::Working;
    let generation = page.generation;
    let outcome = ImportOutcome {
        name: "hollow-knight".to_owned(),
        entry_error: None,
    };
    let task = page.update_with_key_availability(
        Event::ImportFinished(generation, Ok(Box::new(outcome))),
        true,
    );
    drive(&mut page, task, true);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "hollow-knight");
    assert_eq!(calls[0].title, "Hollow Knight");
}

/// Registers a native environment `name` with a desktop target. Creation
/// leaves its entry to whoever reconciles next, so none exists yet.
fn native_environment(paths: &Paths, root: &Path, name: &str) {
    let exe = root.join("game.sh");
    write_script(&exe, "#!/bin/sh\n");
    Environment::create_native_titled(
        paths,
        &GlobalConfig::default(),
        name,
        Some(&exe.display().to_string()),
        Some("Game"),
        true,
        |_| {},
    )
    .unwrap();
}

fn desktop_entries(paths: &Paths) -> usize {
    fs::read_dir(paths.applications_dir())
        .map(|dir| {
            dir.filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "desktop")
            })
            .count()
        })
        .unwrap_or(0)
}

#[test]
fn the_entry_is_reapplied_after_a_fetch_but_not_after_a_skip() {
    for skip in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (mut page, _calls) = reviewed(root.path());
        native_environment(&page.paths, root.path(), "imported");
        assert_eq!(desktop_entries(&page.paths), 0);
        page.imported_name = Some("imported".to_owned());
        page.fetch_art = true;
        let task = page.start_fetch();
        if skip {
            let _ = page.update_with_key_availability(Event::SkipFetch, true);
        }
        drive(&mut page, task, true);
        assert_eq!(page.step, Step::Finished, "skip {skip}");
        assert_eq!(
            desktop_entries(&page.paths),
            usize::from(!skip),
            "skip {skip}"
        );
    }
}

#[test]
fn a_fetch_result_from_an_older_fetch_is_ignored() {
    let root = tempfile::tempdir().unwrap();
    let (mut page, _calls, _task) = fetching(root.path());
    let stale = page.fetch_sequence.wrapping_sub(1);
    let _ =
        page.update_with_key_availability(Event::ArtFetched(stale, (Ok(Vec::new()), Ok(()))), true);
    assert_eq!(page.step, Step::Working);
    assert!(page.fetching);
}

#[test]
fn a_failed_fetch_lands_on_art_failed_and_retry_fetches_again_through_skip() {
    let root = tempfile::tempdir().unwrap();
    let (mut page, _calls) = reviewed(root.path());
    page.imported_name = Some("imported".to_owned());
    page.fetch_art = true;
    let (fetcher, _) = fake_fetcher(Err("offline".to_owned()));
    page.fetcher = fetcher;
    let task = page.start_fetch();
    drive(&mut page, task, true);
    assert_eq!(page.step, Step::ArtFailed);
    assert_eq!(page.error.as_deref(), Some("offline"));
    assert!(page.fetch_failed);
    assert!(!page.busy());

    // Retry runs the fetch, with Skip, since the checkbox was on.
    let (fetcher, retried) = fake_fetcher(Ok(vec!["No logo artwork found".to_owned()]));
    page.fetcher = fetcher;
    let task = page.update_with_key_availability(Event::RetryArt, true);
    assert!(page.fetching);
    assert_eq!(page.step, Step::Working);
    let _ = page.update_with_key_availability(Event::SkipFetch, true);
    drive(&mut page, task, true);
    assert_eq!(call_count(&retried), 1);
    assert_eq!(page.step, Step::Finished);
    assert_eq!(page.notes, ["Artwork skipped"]);

    // A retry that gets through clears the error and keeps the notes.
    page.step = Step::ArtFailed;
    page.error = Some("offline".to_owned());
    let task = page.update_with_key_availability(Event::RetryArt, true);
    drive(&mut page, task, true);
    assert_eq!(page.step, Step::Finished);
    assert!(page.error.is_none());
    assert!(page.notes.contains(&"No logo artwork found".to_owned()));
    assert_eq!(call_count(&retried), 2);
}

#[test]
fn the_failed_art_line_says_what_failed() {
    let root = tempfile::tempdir().unwrap();
    let (mut page, _) = reviewed(root.path());
    page.imported_name = Some("imported".to_owned());
    assert_eq!(
        page.art_failed_line(),
        "imported was imported, but its desktop entry could not be refreshed."
    );
    page.fetch_failed = true;
    assert_eq!(
        page.art_failed_line(),
        "imported was imported, but fetching its artwork failed."
    );
}

#[test]
fn a_failed_entry_refresh_in_the_worker_lands_on_art_failed_and_retry_fetches_first() {
    let root = tempfile::tempdir().unwrap();
    let (mut page, calls) = reviewed(root.path());
    let _ = page.update_with_key_availability(Event::Import, true);
    assert!(page.fetch_art);
    let generation = page.generation;
    let outcome = ImportOutcome {
        name: "imported".to_owned(),
        entry_error: Some("no applications folder".to_owned()),
    };
    let task = page.update_with_key_availability(
        Event::ImportFinished(generation, Ok(Box::new(outcome))),
        true,
    );
    // The entry failed, so the fetch waits for Retry.
    assert_eq!(page.step, Step::ArtFailed);
    assert_eq!(page.error.as_deref(), Some("no applications folder"));
    assert!(!page.fetch_failed);
    assert!(!page.fetching);
    drive(&mut page, task, true);
    assert_eq!(call_count(&calls), 0);

    let task = page.update_with_key_availability(Event::RetryArt, true);
    assert!(page.fetching, "the checkbox was on, so Retry fetches");
    drive(&mut page, task, true);
    assert_eq!(call_count(&calls), 1);
    assert_eq!(page.step, Step::Finished);
}

#[test]
fn retry_without_the_checkbox_only_reapplies_the_entry() {
    let root = tempfile::tempdir().unwrap();
    let (mut page, calls) = reviewed(root.path());
    page.imported_name = Some("imported".to_owned());
    page.fetch_art = false;
    page.step = Step::ArtFailed;
    let task = page.update_with_key_availability(Event::RetryArt, true);
    assert!(!page.fetching);
    assert_eq!(page.step, Step::Working);
    assert!(page.busy());
    drive(&mut page, task, true);
    assert_eq!(page.step, Step::Finished);
    assert_eq!(call_count(&calls), 0);
}

#[test]
fn skip_artwork_on_art_failed_reapplies_the_entry_and_says_what_was_skipped() {
    // What failed was the fetch, never ran, or got through and left the entry.
    for (fetch_art, fetch_done, skipped) in [
        (true, false, true),
        (false, false, false),
        (true, true, false),
    ] {
        let case = format!("fetch {fetch_art}, done {fetch_done}");
        let root = tempfile::tempdir().unwrap();
        let (mut page, calls) = reviewed(root.path());
        native_environment(&page.paths, root.path(), "imported");
        page.imported_name = Some("imported".to_owned());
        page.fetch_art = fetch_art;
        page.fetch_done = fetch_done;
        page.error = Some("offline".to_owned());
        page.step = Step::ArtFailed;
        let task = page.update_with_key_availability(Event::SkipArt, true);
        assert_eq!(page.status, "Refreshing desktop entries...", "{case}");
        assert!(page.error.is_none(), "{case}: the old failure is cleared");
        drive(&mut page, task, true);
        assert_eq!(page.step, Step::Finished, "{case}");
        assert!(page.error.is_none(), "{case}");
        assert_eq!(call_count(&calls), 0, "{case}: Skip artwork never fetches");
        assert_eq!(desktop_entries(&page.paths), 1, "{case}: the entry is back");
        assert_eq!(
            page.notes.contains(&"Artwork skipped".to_owned()),
            skipped,
            "{case}: {:?}",
            page.notes
        );
    }
}

#[test]
fn a_fetch_that_got_through_keeps_its_notes_when_only_the_entry_failed() {
    let root = tempfile::tempdir().unwrap();
    let (mut page, calls) = reviewed(root.path());
    page.imported_name = Some("imported".to_owned());
    page.fetch_art = true;
    let _task = page.start_fetch();
    let sequence = page.fetch_sequence;
    let notes = vec!["No logo artwork found".to_owned()];
    let _ = page.update_with_key_availability(
        Event::ArtFetched(sequence, (Ok(notes.clone()), Err("no folder".to_owned()))),
        true,
    );
    assert_eq!(page.step, Step::ArtFailed);
    assert_eq!(page.error.as_deref(), Some("no folder"));
    assert!(!page.fetch_failed, "the fetch didn't fail");
    assert_eq!(page.notes, notes);
    assert_eq!(
        page.art_failed_line(),
        "imported was imported, but its desktop entry could not be refreshed."
    );

    // Retry has only the entry left, so it doesn't download again.
    let task = page.update_with_key_availability(Event::RetryArt, true);
    assert!(!page.fetching);
    assert_eq!(page.status, "Refreshing desktop entries...");
    drive(&mut page, task, true);
    assert_eq!(page.step, Step::Finished);
    assert_eq!(call_count(&calls), 0);
    assert_eq!(page.notes, notes);
}

#[test]
fn a_failed_fetch_blames_the_fetch_even_when_the_entry_failed_too() {
    let root = tempfile::tempdir().unwrap();
    let (mut page, _) = reviewed(root.path());
    page.imported_name = Some("imported".to_owned());
    page.fetch_art = true;
    let _task = page.start_fetch();
    let sequence = page.fetch_sequence;
    let _ = page.update_with_key_availability(
        Event::ArtFetched(
            sequence,
            (Err("offline".to_owned()), Err("no folder".to_owned())),
        ),
        true,
    );
    assert_eq!(page.step, Step::ArtFailed);
    assert!(page.fetch_failed);
    assert!(!page.fetch_done);
    assert_eq!(
        page.error.as_deref(),
        Some("offline; desktop entry: no folder")
    );
}

#[test]
fn a_failed_or_canceled_import_never_fetches() {
    let root = tempfile::tempdir().unwrap();
    let (mut page, calls) = reviewed(root.path());
    let _ = page.update_with_key_availability(Event::Import, true);
    assert!(page.fetch_art);
    let generation = page.generation;
    let task = page.update_with_key_availability(
        Event::ImportFinished(generation, Err("disk full".to_owned())),
        true,
    );
    assert_eq!(page.step, Step::Review);
    assert_eq!(page.error.as_deref(), Some("disk full"));
    assert!(!page.fetching);
    drive(&mut page, task, true);
    assert_eq!(call_count(&calls), 0);

    // A real import canceled before it starts copying.
    let (mut page, calls) = reviewed(root.path());
    let task = page.update_with_key_availability(Event::Import, true);
    page.cancel(false);
    drive(&mut page, task, true);
    assert_eq!(page.step, Step::Review, "{:?}", page.error);
    assert!(page.error.is_some());
    assert!(!page.paths.environment_config("imported").exists());
    assert!(!page.fetching);
    assert_eq!(call_count(&calls), 0);
}

#[test]
fn ctrl_enter_does_nothing_behind_the_discard_prompt() {
    let root = tempfile::tempdir().unwrap();
    let (mut page, _) = reviewed(root.path());
    page.step = Step::Details;
    page.review = None;
    let _ = page.update_with_key_availability(Event::Name("imported".to_owned()), true);
    let _ = page.back();
    assert!(page.discard_prompt);
    let mut app = app_with_key(page, true);
    let _ = app.update(crate::Message::Key(
        iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
        iced::keyboard::Modifiers::CTRL,
        iced::event::Status::Ignored,
    ));
    let page = prefix_page(&app);
    assert!(page.discard_prompt);
    assert_eq!(
        page.step,
        Step::Details,
        "the review didn't start behind it"
    );
}

/// The Finished page's own column, under the page title, as the probe lays
/// it out: the import line, the targets line, the notes, then the buttons.
const FINISHED_BODY: [usize; 5] = [0, 0, 0, 0, 1];

#[test]
fn the_fetch_notes_show_as_lines_on_finished() {
    use crate::layout_probe::click_at;
    let root = tempfile::tempdir().unwrap();
    let (mut page, _) = reviewed(root.path());
    page.imported_name = Some("imported".to_owned());
    page.step = Step::Finished;
    let size = iced::Size::new(900.0, 720.0);
    let first_button = |page: &Page, row: usize| {
        let view = page.view_with_key_availability(Palette::DARK, true);
        click_at(view, size, &[&FINISHED_BODY[..], &[row, 0]].concat())
    };
    assert_eq!(first_button(&page, 2), ["OpenSettings"]);
    page.notes = vec![
        "No artwork match for imported".to_owned(),
        "No logo artwork found".to_owned(),
    ];
    // Each note is a line of its own, so the buttons sit two children later.
    assert_eq!(first_button(&page, 4), ["OpenSettings"]);
}

#[test]
fn choose_artwork_leaves_for_the_art_page_and_never_launches() {
    let root = tempfile::tempdir().unwrap();
    let (mut page, _) = reviewed(root.path());
    page.imported_name = Some("imported".to_owned());
    page.step = Step::Finished;
    let _ = page.update_with_key_availability(Event::ChooseArt, true);
    assert!(matches!(
        page.take_action(),
        Some(crate::flow::Action::Art { name }) if name == "imported"
    ));

    // Only a finished import has something to choose art for.
    page.step = Step::ArtFailed;
    let _ = page.update_with_key_availability(Event::ChooseArt, true);
    assert!(page.take_action().is_none());
}

#[test]
fn finished_buttons_wrap_in_small_dialogs() {
    use crate::layout_probe::{bounds_at, probe};
    let root = tempfile::tempdir().unwrap();
    let (mut page, _) = reviewed(root.path());
    page.imported_name = Some("imported".to_owned());
    page.notes = vec!["No artwork match for imported".to_owned()];
    page.step = Step::Finished;
    // The title, the target line, the note, then the buttons in a row.
    let row = |child: usize| [&FINISHED_BODY[..], &[3, child]].concat();
    for (width, height) in [(280.0, 240.0), (400.0, 300.0)] {
        let mut view = page.view_with_key_availability(Palette::DARK, true);
        let probe = probe(&mut view, iced::Size::new(width, height));
        let frame = iced::Rectangle::with_size(iced::Size::new(width, height));
        let spilled = probe.outside(frame);
        assert!(spilled.is_empty(), "{width}x{height}: {spilled:?}");
        let buttons: Vec<_> = (0..3)
            .map(|button| bounds_at(&probe.node, &row(button)))
            .collect();
        // Squeezed onto one line, the labels would wrap inside taller buttons.
        for bounds in &buttons {
            assert!(
                (bounds.height - buttons[0].height).abs() < 0.01,
                "{width}x{height}: {buttons:?}"
            );
        }
        assert!(
            buttons[2].y > buttons[0].y,
            "{width}x{height}: the row wraps, {buttons:?}"
        );
    }
}

#[test]
fn the_checkbox_is_on_by_default_and_toggling_it_is_not_an_edit() {
    let root = tempfile::tempdir().unwrap();
    let mut page = Page::new(paths(root.path())).0;
    // Nothing shows until the settings are read.
    assert_eq!(page.art_offer(true), ArtOffer::Unknown);
    page.fetch_kinds = Some(true);
    assert_eq!(page.art_offer(true), ArtOffer::Fetch(true));
    let _ = page.update_with_key_availability(Event::FetchArt(false), true);
    assert_eq!(page.art_offer(true), ArtOffer::Fetch(false));
    let _ = page.update_with_key_availability(Event::FetchArt(true), true);
    assert_eq!(page.art_offer(true), ArtOffer::Fetch(true));
    assert!(!page.dirty);
    // Back asks about nothing, so it leaves for the Add choices.
    let _ = page.back();
    assert!(!page.discard_prompt);
    assert!(matches!(
        page.take_action(),
        Some(crate::flow::Action::Choose)
    ));
}

#[test]
fn no_key_or_no_kinds_shows_a_note_and_never_fetches() {
    let root = tempfile::tempdir().unwrap();
    let (mut page, calls) = reviewed(root.path());
    assert_eq!(page.art_offer(false), ArtOffer::NoKey);
    page.fetch_kinds = Some(false);
    assert_eq!(page.art_offer(true), ArtOffer::Off);
    for key in [false, true] {
        let mut page = reviewed(root.path()).0;
        page.fetch_kinds = Some(false);
        page.step = Step::Review;
        let _ = page.update_with_key_availability(Event::Import, key);
        assert!(!page.fetch_art, "key {key}");
    }
    assert_eq!(call_count(&calls), 0);
}

#[test]
fn a_key_that_vanishes_before_import_turns_the_fetch_off() {
    let root = tempfile::tempdir().unwrap();
    let (mut page, _) = reviewed(root.path());
    assert_eq!(page.art_offer(true), ArtOffer::Fetch(true));
    let _ = page.update_with_key_availability(Event::Import, false);
    assert!(!page.fetch_art);
}

/// The app with a SteamGridDB key, on the Import prefix page that `page` is.
fn app_with_key(page: Page, key: bool) -> crate::App {
    let mut app = crate::tests::app();
    let _ = app.update(crate::Message::SteamGridDbKeyPresenceLoaded(1, key));
    assert_eq!(app.steamgriddb_available, key);
    app.flow = Some(crate::flow::Page::Prefix(Box::new(page)));
    app
}

fn prefix_page(app: &crate::App) -> &Page {
    match &app.flow {
        Some(crate::flow::Page::Prefix(page)) => page,
        _ => panic!("not on the Import prefix page"),
    }
}

#[test]
fn ctrl_enter_on_review_keeps_the_fetch_the_checkbox_shows() {
    for key in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let (page, _) = reviewed(root.path());
        let mut app = app_with_key(page, key);
        // The import's task comes back unrun.
        let _ = app.update(crate::Message::Key(
            iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
            iced::keyboard::Modifiers::CTRL,
            iced::event::Status::Ignored,
        ));
        let page = prefix_page(&app);
        assert_eq!(page.step, Step::Working, "key {key}");
        assert_eq!(
            page.fetch_art, key,
            "Import kept the checkbox as it showed with key {key}"
        );
    }
}

#[test]
fn closing_while_fetching_asks_nothing_and_cancels_the_fetch() {
    let root = tempfile::tempdir().unwrap();
    // The fetch's task is dropped unrun; only its token matters here.
    let (page, _calls, _task) = fetching(root.path());
    let token = page.fetch_token.clone().unwrap();
    let mut app = app_with_key(page, true);
    assert!(!app.management_busy(), "fetching blocks nothing");
    assert!(!app.foreground_busy());
    let _ = app.update(app.window_event(iced::window::Event::CloseRequested));
    assert!(!app.close_prompt);
    assert!(app.exiting);
    assert!(token.is_canceled());
}

#[test]
fn a_close_that_waits_still_cancels_the_fetch() {
    let root = tempfile::tempdir().unwrap();
    let (page, _calls, _task) = fetching(root.path());
    let token = page.fetch_token.clone().unwrap();
    let mut app = app_with_key(page, true);
    // The flow covers the library, so the write started before it opened.
    let flow = app.flow.take();
    let _ = app.update(crate::Message::Sort(
        crate::sort_control::Event::FlipDirection,
    ));
    app.flow = flow;
    assert!(app.library_writes_pending());
    let _ = app.update(app.window_event(iced::window::Event::CloseRequested));
    assert!(app.closing && app.close_quietly && !app.exiting);
    assert!(token.is_canceled());
}

#[test]
fn the_discard_prompt_no_longer_mentions_artwork() {
    use crate::layout_probe::probe;
    assert!(!DISCARD_BODY.contains("artwork"), "{DISCARD_BODY}");
    let root = tempfile::tempdir().unwrap();
    let mut page = Page::new(paths(root.path())).0;
    let _ = page.update_with_key_availability(Event::Name("draft".to_owned()), true);
    let _ = page.back();
    assert!(page.discard_prompt);
    // The prompt still fits the smallest dialog.
    let size = iced::Size::new(280.0, 240.0);
    let mut view = page.view_with_key_availability(Palette::DARK, true);
    let probe = probe(&mut view, size);
    assert!(probe.outside(iced::Rectangle::with_size(size)).is_empty());
}

#[test]
fn details_with_the_checkbox_or_its_note_fit_narrow_and_wide_dialogs() {
    use crate::layout_probe::probe;
    let root = tempfile::tempdir().unwrap();
    let mut page = Page::new(paths(root.path())).0;
    for (label, kinds, key) in [
        ("checkbox", true, true),
        ("no key", true, false),
        ("no kinds", false, true),
    ] {
        page.fetch_kinds = Some(kinds);
        for (width, height) in [(280.0, 240.0), (400.0, 300.0), (900.0, 720.0)] {
            let inset = iced::Rectangle::new(
                iced::Point::new(24.0, 24.0),
                iced::Size::new(width - 48.0, height - 48.0),
            );
            let mut view = page.view_with_key_availability(Palette::DARK, key);
            let probe = probe(&mut view, iced::Size::new(width, height));
            let spilled = probe.outside(inset);
            assert!(
                spilled.is_empty(),
                "{label} at {width}x{height}: {spilled:?}"
            );
        }
    }
}
