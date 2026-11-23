use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::rc::Rc;

use wineo_core::art::Kind;
use wineo_core::config::{Defaults, Hooks, HooksSetting};
use wineo_core::steamgriddb::KeySource;

use super::*;
use crate::test_script::write_script;
use wineo_core::gamescope::{Gamescope, GamescopeSetting};

#[test]
fn template_summary_lists_new_launch_settings() {
    let template = Template {
        mangohud: Some(true),
        wayland: Some(false),
        gamescope: Some(GamescopeSetting::On(Gamescope::default())),
        ..Template::default()
    };
    assert_eq!(
        describe_template(&template),
        "mangohud on, wayland off, gamescope on"
    );
}

#[test]
fn template_summary_lists_configured_hook_phases() {
    let summary = |hooks| {
        describe_template(&Template {
            hooks,
            ..Template::default()
        })
    };

    assert_eq!(
        summary(Some(HooksSetting::On(Hooks {
            before: Some("mount /mnt/games".to_owned()),
            after: None,
        }))),
        "before hook"
    );
    assert_eq!(
        summary(Some(HooksSetting::On(Hooks {
            before: None,
            after: Some("sync".to_owned()),
        }))),
        "after hook"
    );
    assert_eq!(
        summary(Some(HooksSetting::On(Hooks {
            before: Some("mount /mnt/games".to_owned()),
            after: Some("sync".to_owned()),
        }))),
        "before and after hooks"
    );
    assert_eq!(
        summary(Some(HooksSetting::On(Hooks::default()))),
        "sets nothing"
    );
    assert_eq!(summary(Some(HooksSetting::Off)), "sets nothing");
    assert_eq!(summary(None), "sets nothing");
}
#[test]
fn a_launcher_goes_inside_the_runners_parenthesis() {
    let root = tempfile::tempdir().unwrap();
    let script = |path: std::path::PathBuf, body: &str| {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_script(&path, body);
    };
    for (name, version) in [
        ("GE-Proton11-3", "GE-Proton11-3"),
        ("Proton-CachyOS-latest", "cachyos-11.0-20260703-slr"),
    ] {
        script(root.path().join(name).join("proton"), "#!/bin/sh\n");
        fs::write(
            root.path().join(name).join("version"),
            format!("1 {version}\n"),
        )
        .unwrap();
    }
    for (name, body) in [
        ("wine-ge-8-26", "#!/bin/sh\necho wine-8.26\n"),
        ("plain", "#!/bin/sh\nexit 1\n"),
    ] {
        script(root.path().join(name).join("bin/wine"), body);
        script(root.path().join(name).join("bin/wineserver"), "#!/bin/sh\n");
    }
    let runners = Runners::at(root.path().to_owned());
    let row = |name: &str, launcher: Option<&str>| {
        describe_runner_from(&runners.resolve(name).unwrap(), launcher.map(str::to_owned))
    };
    assert_eq!(
        row("GE-Proton11-3", Some("Steam")),
        "GE-Proton11-3 (Proton, Steam)"
    );
    assert_eq!(
        row("Proton-CachyOS-latest", Some("Heroic")),
        "Proton-CachyOS-latest (Proton, cachyos-11.0-20260703-slr, Heroic)"
    );
    assert_eq!(
        row("wine-ge-8-26", Some("Lutris")),
        "wine-ge-8-26 (wine-8.26, Lutris)"
    );
    assert_eq!(row("plain", Some("Steam")), "plain (Steam)");
    // wineo's own builds read as before.
    assert_eq!(row("GE-Proton11-3", None), "GE-Proton11-3 (Proton)");
    assert_eq!(row("wine-ge-8-26", None), "wine-ge-8-26 (wine-8.26)");
    assert_eq!(row("plain", None), "plain");
}

use crate::commands::art::NO_KEY_NOTE;
use crate::commands::art::fake::{Fake, PNG};
use crate::prompt::Script;

struct World {
    _root: tempfile::TempDir,
    paths: Paths,
    global: GlobalConfig,
}

/// A fake wine runner and an existing prefix at the suggested location,
/// so creating `hades` adopts it without building one.
fn world() -> World {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().to_owned();
    let paths =
        Paths::from_lookup(move |key| (key == "HOME").then(|| OsString::from(home.as_os_str())))
            .unwrap();
    let runners = root.path().join("runners");
    let bin = runners.join("fake/bin");
    fs::create_dir_all(&bin).unwrap();
    for executable in ["wine", "wineserver"] {
        write_script(bin.join(executable), "#!/bin/sh\n");
    }
    let prefixes = root.path().join("prefixes");
    fs::create_dir_all(prefixes.join("hades")).unwrap();
    fs::write(prefixes.join("hades/system.reg"), "#arch=win64\n").unwrap();
    let global = GlobalConfig {
        defaults: Defaults {
            runner: "fake".to_owned(),
            runner_root: Some(runners.display().to_string()),
            prefix_root: Some(prefixes.display().to_string()),
            ..Defaults::default()
        },
        ..GlobalConfig::default()
    };
    World {
        _root: root,
        paths,
        global,
    }
}

fn fake() -> Fake {
    let mut fake = Fake::default();
    for term in ["hades", "Hades"] {
        fake.search(term, true);
    }
    fake.page(Kind::Icon, &[1]);
    fake.0
        .insert("https://cdn.example/1".into(), (200, PNG.to_vec()));
    for kind in [Kind::Grid, Kind::Hero, Kind::Logo] {
        fake.page(kind, &[]);
    }
    fake
}

/// Runs guided `new` on `answers`, returning the result, the transcript,
/// and how many times the key was looked up.
fn wizard(
    world: &World,
    key: fn() -> KeyCheck,
    answers: &[&str],
) -> (anyhow::Result<ExitCode>, String, usize) {
    let lookups = Cell::new(0);
    let fake = fake();
    let access = Access {
        check_key: Box::new(|| {
            lookups.set(lookups.get() + 1);
            key()
        }),
        transport: Box::new(move || Ok(Box::new(fake.clone()))),
    };
    let mut prompt = Prompt::new(Script::new(answers), Vec::new());
    let mut guided = Guided { registered: None };
    let result = run(
        &world.paths,
        &world.global,
        &Prefill::default(),
        &mut prompt,
        &mut guided,
        &access,
    );
    let transcript = String::from_utf8_lossy(prompt.output()).into_owned();
    (result, transcript, lookups.get())
}

fn found() -> KeyCheck {
    KeyCheck::Found(Key {
        value: "k".into(),
        source: KeySource::Variable,
    })
}

fn missing() -> KeyCheck {
    KeyCheck::Missing
}

fn saved_game(world: &World) -> Option<u64> {
    wineo_core::art::saved_game(&world.paths, "hades").unwrap()
}

// Enter picks Wine at the kind question.
const BEFORE_ART: [&str; 6] = ["", "1", "hades", "", "1", "3"];

fn answers(rest: &[&'static str]) -> Vec<&'static str> {
    BEFORE_ART.iter().chain(rest).copied().collect()
}

#[test]
fn no_key_notes_it_once_and_asks_nothing() {
    let world = world();
    // Back from the review goes to How, then forward again.
    let (result, transcript, lookups) = wizard(&world, missing, &answers(&["b", "3", ""]));
    assert!(result.is_ok(), "{result:?}\n{transcript}");
    assert_eq!(lookups, 1);
    assert_eq!(transcript.matches(NO_KEY_NOTE).count(), 1, "{transcript}");
    assert!(!transcript.contains("Fetch art"), "{transcript}");
    assert!(transcript.contains("\n  art       none\n"), "{transcript}");
    assert_eq!(transcript.matches("How does hades").count(), 2);
    assert!(world.paths.environment_config("hades").exists());
    assert_eq!(saved_game(&world), None);
}

#[test]
fn a_failed_key_lookup_warns_and_skips() {
    let world = world();
    let failed = || KeyCheck::Failed(anyhow::anyhow!("the keyring failed: dismissed"));
    let (result, transcript, _) = wizard(&world, failed, &answers(&[""]));
    assert!(result.is_ok(), "{result:?}\n{transcript}");
    assert!(
        transcript.contains("warning: the keyring failed: dismissed, skipping art\n"),
        "{transcript}"
    );
    assert!(!transcript.contains("Fetch art"), "{transcript}");
}

#[test]
fn yes_searches_the_term_after_creating() {
    let world = world();
    let (result, transcript, _) = wizard(&world, found, &answers(&["1", "Hades", "", "1"]));
    assert!(result.is_ok(), "{result:?}\n{transcript}");
    assert!(transcript.contains("Fetch art from SteamGridDB?\n  1) Yes\n  2) No\n"));
    assert!(
        transcript.contains("Art [1]: Search for [hades]: "),
        "{transcript}"
    );
    assert!(
        transcript.contains("\n  art       fetch from SteamGridDB, searching \"Hades\"\n"),
        "{transcript}"
    );
    let created = transcript.find("Created hades").unwrap();
    let picked = transcript.find("Which game is it?").unwrap();
    assert!(created < picked, "{transcript}");
    assert!(
        transcript.contains("icon: fetched SteamGridDB 1\n"),
        "{transcript}"
    );
    assert_eq!(saved_game(&world), Some(7));
    assert!(world.paths.art_dir("hades").join("icon.png").exists());
    assert!(!transcript.contains("Art not fetched"), "{transcript}");
}

#[test]
fn an_installer_that_cant_be_prepared_says_the_art_was_not_fetched() {
    let world = world();
    install_runner(&world);
    let program = program(&world);
    // The installer goes away while the environment is being created.
    write_script(
        world._root.path().join("runners/fake/bin/wine"),
        format!("#!/bin/sh\ncase \"$*\" in\n*reg*) rm -f '{program}' ;;\nesac\nexit 0\n"),
    );
    let answers = art_after(&["1", &program], &["1"]);
    let (result, transcript, seen) = wizard_probed(&world, &borrowed(&answers));
    assert!(
        matches!(&result, Ok(code) if *code == ExitCode::FAILURE),
        "{result:?}\n{transcript}"
    );
    assert_eq!(
        transcript
            .matches("Art not fetched. Run `wineo art fetch hades` to fetch it.\n")
            .count(),
        1,
        "{transcript}"
    );
    // Only the search ran, and nothing was stored.
    assert_eq!(seen.len(), 1, "{transcript}");
    assert_eq!(saved_game(&world), None);
}

#[test]
fn creation_goes_on_with_the_config_it_read_rather_than_the_one_at_start() {
    let world = world();
    // Changed on disk after the wizard loaded its config, to fetch only
    // grids, which the fake has none of.
    let config = world.paths.global_config();
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "[art]\nfetch = [\"grid\"]\n").unwrap();
    let (result, transcript, _) = wizard(&world, found, &answers(&["1", "Hades", "", "1"]));
    assert!(result.is_ok(), "{result:?}\n{transcript}");
    assert!(transcript.contains("Created hades"), "{transcript}");
    assert!(
        !world.paths.art_dir("hades").join("icon.png").exists(),
        "{transcript}"
    );
}

#[test]
fn no_fetches_nothing() {
    let world = world();
    let (result, transcript, _) = wizard(&world, found, &answers(&["2", ""]));
    assert!(result.is_ok(), "{result:?}\n{transcript}");
    assert!(transcript.contains("\n  art       none\n"), "{transcript}");
    assert!(!transcript.contains("Which game is it?"), "{transcript}");
    assert_eq!(saved_game(&world), None);
}

#[test]
fn back_moves_between_how_art_and_the_review() {
    let world = world();
    let (result, transcript, lookups) = wizard(
        &world,
        found,
        // Art back to How, then yes, the search back to Art, yes again with
        // a term, the review back to Art keeping it, back again, and no.
        &answers(&["b", "3", "1", "<", "1", "Hades", "b", "1", "", "b", "2", ""]),
    );
    assert!(result.is_ok(), "{result:?}\n{transcript}");
    assert_eq!(lookups, 1);
    assert_eq!(transcript.matches("How does hades").count(), 2);
    assert_eq!(transcript.matches("Fetch art from SteamGridDB?").count(), 5);
    // After a yes the question defaults to yes, and the search to the term.
    assert!(transcript.contains("Search for [Hades]: "), "{transcript}");
    assert!(transcript.contains("searching \"Hades\""), "{transcript}");
    let last = transcript.rfind("Ready to create hades:").unwrap();
    assert!(
        transcript[last..].contains("\n  art       none\n"),
        "{transcript}"
    );
    assert_eq!(saved_game(&world), None);
}

#[test]
fn the_skip_entry_fetches_nothing() {
    let world = world();
    let (result, transcript, _) = wizard(&world, found, &answers(&["1", "", "", "4"]));
    assert!(result.is_ok(), "{result:?}\n{transcript}");
    assert!(
        transcript.contains("  3) Search for something else\n"),
        "{transcript}"
    );
    assert!(
        transcript.contains("  4) None of these, skip art\n"),
        "{transcript}"
    );
    assert!(
        transcript
            .contains("Skipped art. Fetch later with: wineo art fetch hades --search <term>\n"),
        "{transcript}"
    );
    assert_eq!(saved_game(&world), None);
    assert!(world.paths.environment_config("hades").exists());
}

#[test]
fn canceling_the_pick_list_keeps_the_environment() {
    let world = world();
    let (result, transcript, _) = wizard(&world, found, &answers(&["1", "", "", "q"]));
    assert!(
        result.as_ref().is_err_and(|err| err.is::<Canceled>()),
        "{result:?}\n{transcript}"
    );
    assert!(world.paths.environment_config("hades").exists());
}

/// What `wizard` returns, plus each request SteamGridDB saw with whether the
/// environment existed and its lock was free.
fn wizard_probed(
    world: &World,
    answers: &[&str],
) -> (
    anyhow::Result<ExitCode>,
    String,
    Vec<crate::commands::art::fake::Seen>,
) {
    use crate::commands::art::fake::Probe;
    let seen = Rc::new(RefCell::new(Vec::new()));
    let fake = fake();
    let access = Access {
        check_key: Box::new(found),
        transport: Box::new(|| {
            Ok(Box::new(Probe {
                fake: fake.clone(),
                paths: world.paths.clone(),
                name: "hades".to_owned(),
                runner: "fake".to_owned(),
                seen: seen.clone(),
            }))
        }),
    };
    let mut prompt = Prompt::new(Script::new(answers), Vec::new());
    let mut guided = Guided { registered: None };
    let result = run(
        &world.paths,
        &world.global,
        &Prefill::default(),
        &mut prompt,
        &mut guided,
        &access,
    );
    let transcript = String::from_utf8_lossy(prompt.output()).into_owned();
    drop(access);
    let seen = Rc::try_unwrap(seen).ok().unwrap().into_inner();
    (result, transcript, seen)
}

/// The fake runner where the installer step looks for it, since that step
/// reads the config from disk rather than the world's.
fn install_runner(world: &World) {
    let bin = world.paths.default_runner_root().join("fake/bin");
    fs::create_dir_all(&bin).unwrap();
    for executable in ["wine", "wineserver"] {
        write_script(bin.join(executable), "#!/bin/sh\nexit 0\n");
    }
}

/// A file the How question accepts, as a program to use or an installer.
fn program(world: &World) -> String {
    let path = world._root.path().join("game.exe");
    fs::write(&path, "").unwrap();
    path.display().to_string()
}

/// The questions up to How, then How's own answers, then art answered yes
/// with a search for Hades, and the review.
fn art_after(how: &[&str], rest: &[&str]) -> Vec<String> {
    BEFORE_ART[..5]
        .iter()
        .chain(how)
        .chain(&["1", "Hades", ""])
        .chain(rest)
        .map(|answer| (*answer).to_owned())
        .collect()
}

fn borrowed(answers: &[String]) -> Vec<&str> {
    answers.iter().map(String::as_str).collect()
}

#[test]
fn guided_art_without_an_installer_is_fetched_with_the_lock_free() {
    for existing in [false, true] {
        let world = world();
        let program = program(&world);
        let answers = if existing {
            art_after(&["2", &program], &["1", "", ""])
        } else {
            art_after(&["3"], &["1"])
        };
        let (result, transcript, seen) = wizard_probed(&world, &borrowed(&answers));
        assert!(result.is_ok(), "{result:?}\n{transcript}");
        assert!(!seen.is_empty(), "{transcript}");
        for request in &seen {
            assert!(
                request.exists && request.lock_free && request.build_free,
                "{}",
                request.url
            );
        }
        assert_eq!(saved_game(&world), Some(7));
    }
}

#[test]
fn guided_art_is_held_to_the_environment_creation_returned() {
    for (existing, replaced) in [(false, false), (false, true), (true, false), (true, true)] {
        let world = world();
        let program = program(&world);
        let answers = if existing {
            art_after(&["2", &program], &["1", "", ""])
        } else {
            art_after(&["3"], &["1"])
        };
        let fake = fake();
        let access = Access {
            check_key: Box::new(found),
            // The art connects once the environment is made, and a fresh
            // read there would take a replacement for it.
            transport: Box::new(|| {
                if replaced {
                    crate::commands::art::fake::replace_environment(&world.paths, "hades");
                }
                Ok(Box::new(fake.clone()))
            }),
        };
        let answers = borrowed(&answers);
        let mut prompt = Prompt::new(Script::new(&answers), Vec::new());
        let mut guided = Guided { registered: None };
        let result = run(
            &world.paths,
            &world.global,
            &Prefill::default(),
            &mut prompt,
            &mut guided,
            &access,
        );
        let transcript = String::from_utf8_lossy(prompt.output()).into_owned();
        assert!(result.is_ok(), "{result:?}\n{transcript}");
        let icon = world.paths.art_dir("hades").join("icon.png").exists();
        assert_eq!(icon, !replaced, "{existing} {replaced}\n{transcript}");
        assert_eq!(saved_game(&world), (!replaced).then_some(7));
    }
}

#[test]
fn with_an_installer_the_pick_list_comes_before_creation_and_the_download_after() {
    let world = world();
    install_runner(&world);
    let program = program(&world);
    let answers = art_after(&["1", &program], &["1", "s"]);
    let (result, transcript, seen) = wizard_probed(&world, &borrowed(&answers));
    assert!(result.is_ok(), "{result:?}\n{transcript}");
    let picked = transcript.find("Which game is it?").unwrap();
    let created = transcript.find("Created hades").unwrap();
    assert!(picked < created, "{transcript}");
    let (search, downloads) = seen.split_first().expect("the search is asked");
    assert!(search.url.contains("/search/"), "{}", search.url);
    assert!(!search.exists, "nothing exists while the questions run");
    assert!(!downloads.is_empty(), "{transcript}");
    for request in downloads {
        assert!(
            request.exists && request.lock_free && request.build_free,
            "{}",
            request.url
        );
    }
    assert_eq!(saved_game(&world), Some(7));
    assert!(world.paths.art_dir("hades").join("icon.png").exists());
}

#[test]
fn canceling_the_pick_list_before_an_installer_creates_nothing() {
    let world = world();
    let program = program(&world);
    let answers = art_after(&["1", &program], &["q"]);
    let (result, transcript, _) = wizard_probed(&world, &borrowed(&answers));
    assert!(
        result.as_ref().is_err_and(|err| err.is::<Canceled>()),
        "{result:?}\n{transcript}"
    );
    assert!(!world.paths.environment_config("hades").exists());
}

/// Reports `outcome` as the end of the installer for `hades` with a game
/// already picked, returning the result, the transcript, and how many
/// requests SteamGridDB got afterwards.
fn installer_ended_with(
    outcome: wineo_core::Result<Outcome>,
    resolved: bool,
) -> (Ran, String, usize) {
    let (ran, transcript, requests, _) = installer_ended_replacing(outcome, resolved, false);
    (ran, transcript, requests)
}

/// `installer_ended_with`, also returning what went to stderr, with the
/// environment replaced after the identity was read when `replace` is set.
fn installer_ended_replacing(
    outcome: wineo_core::Result<Outcome>,
    resolved: bool,
    replace: bool,
) -> (Ran, String, usize, String) {
    use crate::commands::art::fake::Probe;
    let world = world();
    // The environment the installer ran in, for the art to be stored in.
    let config = world.paths.environment_config("hades");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(
        &config,
        "prefix = 'prefixes/hades'\nrunner = 'fake'\narch = 'win64'\n",
    )
    .unwrap();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let fake = fake();
    let access = Access {
        check_key: Box::new(found),
        transport: Box::new(|| {
            Ok(Box::new(Probe {
                fake: fake.clone(),
                paths: world.paths.clone(),
                name: "hades".to_owned(),
                runner: "fake".to_owned(),
                seen: seen.clone(),
            }))
        }),
    };
    let key = Key {
        value: "k".into(),
        source: KeySource::Variable,
    };
    let request = AutoFetch {
        env: "hades",
        term: "hades",
        kinds: world.global.art.fetch,
    };
    let mut ignored = Vec::new();
    let picked = access
        .pick(
            &key,
            &request,
            Pick::<Script, Vec<u8>>::Top(&mut ignored),
            &mut Vec::new(),
        )
        .unwrap()
        .expect("a game is picked");
    let before = seen.borrow().len();
    let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
    let art = ArtAfter {
        access: &access,
        key: &key,
        picked: &picked,
        kinds: world.global.art.fetch,
        identity: EnvironmentIdentity::acquire(&world.paths, "hades").unwrap(),
    };
    if replace {
        crate::commands::art::fake::replace_environment(&world.paths, "hades");
    }
    let mut err = Vec::new();
    let ran = installer_ended(
        &world.paths,
        "hades",
        &mut prompt,
        (outcome, resolved),
        None,
        Some(art),
        &mut err,
    )
    .unwrap();
    let transcript = String::from_utf8_lossy(prompt.output()).into_owned();
    let requests = seen.borrow().len() - before;
    (ran, transcript, requests, String::from_utf8(err).unwrap())
}

fn assert_ended_without_art((ran, transcript, requests): (Ran, String, usize)) {
    assert!(matches!(ran, Ran::Ended(_)));
    assert_eq!(
        transcript
            .matches("Art not fetched. Run `wineo art fetch hades` to fetch it.\n")
            .count(),
        1,
        "{transcript}"
    );
    assert!(!transcript.contains("icon:"), "{transcript}");
    assert_eq!(requests, 0);
}

fn unsettled() -> wineo_core::Error {
    wineo_core::Error::Unsettled("it could not be confirmed".to_owned())
}

#[test]
fn a_canceled_installer_ends_setup_and_says_the_art_was_not_fetched() {
    assert_ended_without_art(installer_ended_with(
        Ok(Outcome::Canceled { exit_code: None }),
        true,
    ));
}

#[test]
fn a_failed_installer_start_ends_setup_and_says_the_art_was_not_fetched() {
    assert_ended_without_art(installer_ended_with(Err(unsettled()), true));
}

#[test]
fn an_unresolved_installer_ends_setup_and_says_the_art_was_not_fetched() {
    assert_ended_without_art(installer_ended_with(
        Ok(Outcome::Unresolved {
            exit_code: None,
            canceled: false,
            error: unsettled(),
        }),
        true,
    ));
}

#[test]
fn a_finished_installer_downloads_the_art_and_says_nothing_more() {
    let (ran, transcript, requests) =
        installer_ended_with(Ok(Outcome::Finished { exit_code: Some(0) }), true);
    assert!(matches!(ran, Ran::Finished));
    assert!(!transcript.contains("Art not fetched"), "{transcript}");
    assert!(requests > 0);
    assert!(
        transcript.contains("icon: fetched SteamGridDB 1\n"),
        "{transcript}"
    );
}

#[test]
fn a_finished_installer_with_a_failing_exit_still_downloads_the_art() {
    let (ran, transcript, requests) =
        installer_ended_with(Ok(Outcome::Finished { exit_code: Some(1) }), true);
    assert!(matches!(ran, Ran::Finished));
    assert!(
        transcript.contains("The installer exited with code 1.\n"),
        "{transcript}"
    );
    assert!(requests > 0);
    assert!(
        transcript.contains("icon: fetched SteamGridDB 1\n"),
        "{transcript}"
    );
    assert!(!transcript.contains("Art not fetched"), "{transcript}");
}

#[test]
fn a_stopped_step_ends_setup_and_says_the_art_was_not_fetched() {
    let world = world();
    let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
    let step = Stepped::Stopped(anyhow::anyhow!("stopped by a hook"));
    let ControlFlow::Break(ended) =
        ended_early(&world.paths, "hades", &mut prompt, step, true).unwrap()
    else {
        panic!("a stopped step ends setup");
    };
    assert_eq!(ended.unwrap_err().to_string(), "stopped by a hook");
    let transcript = String::from_utf8_lossy(prompt.output()).into_owned();
    assert_eq!(
        transcript
            .matches("Art not fetched. Run `wineo art fetch hades` to fetch it.\n")
            .count(),
        1,
        "{transcript}"
    );
    // Without a pick there is nothing to say.
    let mut prompt = Prompt::new(Script::new(&[]), Vec::new());
    let step = Stepped::Stopped(anyhow::anyhow!("stopped by a hook"));
    let _ = ended_early(&world.paths, "hades", &mut prompt, step, false).unwrap();
    assert_eq!(prompt.output(), b"");
}

#[test]
fn art_is_not_stored_in_an_environment_replaced_before_the_download() {
    let (ran, transcript, _, err) =
        installer_ended_replacing(Ok(Outcome::Finished { exit_code: Some(0) }), true, true);
    assert!(matches!(ran, Ran::Finished));
    assert_eq!(
        err,
        "warning: art fetch failed: environment `hades` changed before artwork could be written\n"
    );
    assert!(!transcript.contains("icon:"), "{transcript}");
    assert!(!transcript.contains("Art not fetched"), "{transcript}");
}

#[test]
fn guided_art_is_not_stored_in_an_environment_replaced_during_the_installer() {
    let world = world();
    install_runner(&world);
    let program = program(&world);
    // The installer replaces the environment, as removing it and creating
    // another of the same name would.
    let swap = format!(
        "#!/bin/sh\ncase \"$*\" in\n*game.exe*) {} ;;\nesac\nexit 0\n",
        crate::commands::art::fake::replace_environment_command(&world.paths, "hades")
    );
    for root in [
        world.paths.default_runner_root(),
        world._root.path().join("runners"),
    ] {
        write_script(root.join("fake/bin/wine"), &swap);
    }
    let answers = art_after(&["1", &program], &["1", "s"]);
    let (result, transcript, _) = wizard_probed(&world, &borrowed(&answers));
    assert!(result.is_ok(), "{result:?}\n{transcript}");
    assert!(!transcript.contains("icon:"), "{transcript}");
    assert!(!world.paths.art_dir("hades").join("icon.png").exists());
    assert_eq!(saved_game(&world), None);
}
