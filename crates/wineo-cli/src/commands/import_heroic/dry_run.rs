use std::io::Write;

use wineo_core::components::{Builds, Component};
use wineo_core::config::GlobalConfig;
use wineo_core::error::Error;
use wineo_core::heroic_import::{Entry, Game, LibraryPlan};
use wineo_core::paths::Paths;
use wineo_core::steamgriddb::{Api, Game as ArtGame};

use super::{Access, ArtChoice};
use crate::style;
use crate::text;

#[cfg(test)]
use crate::snapshot;

pub(super) fn art(
    access: &Access,
    choice: &ArtChoice,
    env: &str,
    out: &mut dyn Write,
    err: &mut dyn Write,
) {
    let Some(key) = choice.key.as_ref().filter(|_| choice.enabled) else {
        return;
    };
    let result = (|| -> anyhow::Result<()> {
        let transport = (access.transport)()?;
        let games = Api::new(&*transport, &key.value).search(&choice.term)?;
        let Some(top) = games.first() else {
            anyhow::bail!("no SteamGridDB games match `{}`", choice.term);
        };
        writeln!(
            out,
            "{env}: art from SteamGridDB game {}, {}",
            top.id,
            game_name(top)
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = writeln!(
            err,
            "{}",
            style::warning_stderr(format!("warning: art fetch failed: {error:#}"))
        );
    }
}

fn game_name(game: &ArtGame) -> String {
    match game.year {
        Some(year) => format!("{} ({year})", game.name),
        None => game.name.clone(),
    }
}

pub(super) fn library(
    paths: &Paths,
    global: &GlobalConfig,
    access: &Access,
    library: &LibraryPlan,
    fetch_art: bool,
    yes: bool,
) -> anyhow::Result<bool> {
    let backfills = library.backfills();
    if !backfills.is_empty() {
        let names = backfills
            .iter()
            .map(|backfill| backfill.environment.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let noun = text::plural(backfills.len(), "environment", "environments");
        println!(
            "Would record the Heroic id for {} existing {noun}: {names}",
            backfills.len()
        );
    }
    for (component, versions) in if yes {
        Vec::new()
    } else {
        missing_builds(paths, global, library)?
    } {
        for (version, count) in versions {
            let build = version.map_or_else(
                || format!("the latest {}", component.label()),
                |version| format!("{} {version}", component.label()),
            );
            let noun = text::plural(count, "game", "games");
            let target = text::plural(count, "it", "them");
            println!(
                "Would offer to download {build} for {count} {noun}. Declining skips {target}."
            );
        }
    }
    let key = if fetch_art && !global.art.fetch.kinds().is_empty() {
        let check = (access.check_key)();
        check.print_skip();
        ArtChoice::checked(check, "").key
    } else {
        None
    };
    let mut failed = false;
    for entry in library.entries() {
        let (name, title, checked) = match entry {
            Entry::Ready(Game::Windows(plan)) => (
                plan.name(),
                plan.heroic().title.as_str(),
                super::super::import::check_review(
                    paths,
                    global,
                    plan.request(),
                    plan.inspection(),
                ),
            ),
            Entry::Pending(game) if !yes => {
                // Accepting the build offer would make this game ready to import.
                let plan = game.plan();
                (
                    plan.name(),
                    plan.heroic().title.as_str(),
                    super::super::import::check_review(
                        paths,
                        global,
                        plan.request(),
                        plan.inspection(),
                    ),
                )
            }
            Entry::Ready(Game::Native(plan)) => (
                plan.name(),
                plan.heroic().title.as_str(),
                super::super::import::check_name(paths, plan.name()),
            ),
            Entry::Pending(_) | Entry::Skipped(_) => continue,
        };
        if let Err(error) = checked {
            println!("{name}: would fail: {error:#}");
            failed = true;
            continue;
        }
        art(
            access,
            &ArtChoice {
                key: key.clone(),
                enabled: key.is_some(),
                term: title.to_owned(),
            },
            name,
            &mut crate::output::stdout(),
            &mut crate::output::stderr(),
        );
    }
    Ok(failed)
}

type BuildCounts = Vec<(Component, Vec<(Option<String>, usize)>)>;

fn missing_builds(
    paths: &Paths,
    global: &GlobalConfig,
    library: &LibraryPlan,
) -> anyhow::Result<BuildCounts> {
    let mut builds = Vec::new();
    for component in Component::ALL {
        let mut versions: Vec<(Option<String>, usize)> = Vec::new();
        for entry in library.entries() {
            let Entry::Pending(game) = entry else {
                continue;
            };
            let plan = game.plan();
            let wanted = plan.wanted();
            let Some(asked) = wanted.get(component) else {
                continue;
            };
            // Checking each component also finds builds hidden behind the first missing one.
            let version = match Builds::new(paths, global, component)?.resolve(asked) {
                Ok(_) => continue,
                Err(Error::NoBuilds { .. }) => None,
                Err(Error::UnknownBuild { name, .. }) => Some(name),
                Err(error) => return Err(error.into()),
            };
            match versions
                .iter_mut()
                .find(|(existing, _)| *existing == version)
            {
                Some((_, count)) => *count += 1,
                None => versions.push((version, 1)),
            }
        }
        if !versions.is_empty() {
            builds.push((component, versions));
        }
    }
    Ok(builds)
}

#[cfg(test)]
mod tests {
    use crate::test_script::write_script;
    use std::cell::RefCell;
    use std::fs;
    use std::io::{Cursor, Read};

    use std::path::Path;
    use std::rc::Rc;

    use wineo_core::download::{Response, Transport};
    use wineo_core::steamgriddb::{self, KeySource, Keyring, KeyringError};

    use super::super::{Args, KeyCheck, run_with_access};
    use super::*;

    struct ReadOnlyKeyring;

    impl Keyring for ReadOnlyKeyring {
        fn get(&self) -> Result<Option<String>, KeyringError> {
            Ok(None)
        }

        fn set(&self, _: &str) -> Result<(), KeyringError> {
            panic!("dry run must not write the keyring")
        }

        fn delete(&self) -> Result<bool, KeyringError> {
            panic!("dry run must not write the keyring")
        }
    }

    #[derive(Clone, Copy)]
    enum Answer {
        Matches,
        Empty,
        Rejected,
        Offline,
    }

    struct Search {
        answer: Answer,
        calls: Rc<RefCell<Vec<String>>>,
    }

    impl Transport for Search {
        fn get(&self, _: &str) -> anyhow::Result<Box<dyn Read>> {
            panic!("dry run must not download images")
        }

        fn request(&self, url: &str, bearer: Option<&str>) -> anyhow::Result<Response> {
            assert_eq!(bearer, Some("k"));
            assert!(
                url.starts_with(&format!("{}/search/autocomplete/", steamgriddb::API)),
                "{url}"
            );
            self.calls.borrow_mut().push(url.to_owned());
            let (status, data) = match self.answer {
                Answer::Matches => (
                    200,
                    r#"[{"id":7,"name":"Hades","release_date":1600000000},{"id":8,"name":"Hades II"},{"id":9,"name":"Hades III"},{"id":10,"name":"Hades IV"}]"#,
                ),
                Answer::Empty => (200, "[]"),
                Answer::Rejected => (401, "[]"),
                Answer::Offline => anyhow::bail!("offline"),
            };
            Ok(Response {
                status,
                body: Box::new(Cursor::new(
                    format!(r#"{{"success":true,"data":{data}}}"#).into_bytes(),
                )),
            })
        }
    }

    fn paths(root: &Path) -> Paths {
        Paths::from_lookup(|key| (key == "HOME").then(|| root.as_os_str().to_owned())).unwrap()
    }

    fn access<'a>(paths: &'a Paths, answer: Answer, calls: Rc<RefCell<Vec<String>>>) -> Access<'a> {
        Access {
            check_key: Box::new(move || {
                match steamgriddb::find_key(paths, |_| None, &ReadOnlyKeyring).unwrap() {
                    Some(key) => {
                        assert_eq!(key.source, KeySource::File);
                        KeyCheck::Found(key)
                    }
                    None => KeyCheck::Missing,
                }
            }),
            transport: Box::new(move || {
                Ok(Box::new(Search {
                    answer,
                    calls: calls.clone(),
                }))
            }),
        }
    }

    fn save_key(paths: &Paths) {
        let file = steamgriddb::key_file(paths);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, "k\n").unwrap();
    }

    fn bulk_fixture(root: &Path, pending: bool, conflict: bool) -> (Paths, std::path::PathBuf) {
        let paths = paths(root);
        save_key(&paths);
        let heroic = root.join("heroic");
        fs::create_dir_all(heroic.join("sideload_apps")).unwrap();
        fs::create_dir_all(heroic.join("GamesConfig")).unwrap();
        let prefix = root.join("source");
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        fs::write(
            prefix.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        let exe = prefix.join("drive_c/game.exe");
        fs::write(&exe, "game").unwrap();
        let marker = root.join("runner-started");
        let runner = paths.host_data().join("runners/test-wine/bin");
        fs::create_dir_all(&runner).unwrap();
        for program in ["wine", "wineserver"] {
            let file = runner.join(program);
            write_script(
                &file,
                format!(
                    "#!/bin/sh\nif [ \"${{1-}}\" = --version ]; then echo wine-test; exit 0; fi\ntouch '{}'\nexit 1\n",
                    marker.display()
                ),
            );
        }
        fs::write(heroic.join("config.json"), format!(r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#, prefix.display())).unwrap();
        let settings = if pending {
            r#""autoInstallDxvk":true, "autoInstallVkd3d":true"#
        } else {
            ""
        };
        fs::write(
            heroic.join("GamesConfig/a.json"),
            format!(r#"{{"a":{{{}}}}}"#, settings),
        )
        .unwrap();
        fs::write(heroic.join("GamesConfig/b.json"), r#"{"b":{}}"#).unwrap();
        fs::write(heroic.join("sideload_apps/library.json"), format!(r#"{{"games":[{{"app_name":"a","title":"Alpha","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}},{{"app_name":"b","title":"Beta","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#, exe.display(), exe.display())).unwrap();
        if conflict {
            fs::create_dir_all(paths.host_data().join("prefixes/alpha")).unwrap();
        }
        (paths, heroic)
    }

    #[test]
    fn bulk_art_skips_pending_games_only_with_yes_and_continues_after_failures() {
        for pending in [false, true] {
            for yes in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let (paths, heroic) = bulk_fixture(root.path(), pending, !pending);
                let calls = Rc::default();
                let access = access(&paths, Answer::Matches, Rc::clone(&calls));
                let args = Args {
                    config_dir: Some(heroic.display().to_string()),
                    runner: Some("test-wine".to_owned()),
                    all: true,
                    yes,
                    art: true,
                    dry_run: true,
                    ..Args::default()
                };
                let before = snapshot::Snapshot::capture(root.path()).unwrap();
                let code = run_with_access(&paths, args, false, &access, true).unwrap();
                before.assert_unchanged(root.path());
                assert!(!root.path().join("runner-started").exists());
                assert_eq!(
                    code,
                    if pending {
                        std::process::ExitCode::SUCCESS
                    } else {
                        std::process::ExitCode::FAILURE
                    }
                );
                let mut expected = vec![format!("{}/search/autocomplete/Beta", steamgriddb::API)];
                if pending && !yes {
                    expected.insert(0, format!("{}/search/autocomplete/Alpha", steamgriddb::API));
                }
                assert_eq!(*calls.borrow(), expected);
            }
        }
    }

    #[test]
    fn missing_builds_preserves_pinned_versions_and_counts_without_writing() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let pending = || {
            super::super::tests::pending_game_wanting(
                root.path(),
                Component::Dxvk,
                "missing",
                Some("dxvk-2.7"),
                Some("vkd3d-proton-2.14"),
                Some("dxvk-2.7"),
            )
        };
        let library = LibraryPlan::from_entries(
            Some("wine-tkg".to_owned()),
            Vec::new(),
            vec![pending(), pending()],
        );
        let before = snapshot::Snapshot::capture(root.path()).unwrap();
        assert_eq!(
            missing_builds(&paths, &GlobalConfig::default(), &library).unwrap(),
            vec![
                (Component::Dxvk, vec![(Some("dxvk-2.7".to_owned()), 2)]),
                (
                    Component::Vkd3d,
                    vec![(Some("vkd3d-proton-2.14".to_owned()), 2)]
                ),
            ]
        );
        before.assert_unchanged(root.path());
    }

    #[test]
    fn art_lists_only_three_matches_and_marks_the_automatic_pick_without_writing() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        save_key(&paths);
        let calls = Rc::default();
        let access = access(&paths, Answer::Matches, Rc::clone(&calls));
        let choice = ArtChoice::checked((access.check_key)(), "Heroic title");
        let before = snapshot::Snapshot::capture(root.path()).unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        access.review(
            &choice.term,
            GlobalConfig::default().art.fetch,
            &mut out,
            &mut err,
        );
        before.assert_unchanged(root.path());
        assert!(err.is_empty());
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Art from SteamGridDB, searching \"Heroic title\":\n  1. Hades (2020), game 7 <- would be picked automatically\n  2. Hades II, game 8\n  3. Hades III, game 9\n"
        );
        assert_eq!(
            calls.borrow().as_slice(),
            [format!(
                "{}/search/autocomplete/Heroic%20title",
                steamgriddb::API
            )]
        );
    }

    #[test]
    fn bulk_art_prints_the_real_top_match_line_without_writing() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        save_key(&paths);
        let calls = Rc::default();
        let access = access(&paths, Answer::Matches, calls);
        let choice = ArtChoice::checked((access.check_key)(), "Alpha");
        let before = snapshot::Snapshot::capture(root.path()).unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        art(&access, &choice, "alpha", &mut out, &mut err);
        before.assert_unchanged(root.path());
        assert!(err.is_empty());
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "alpha: art from SteamGridDB game 7, Hades (2020)\n"
        );
    }

    #[test]
    fn dry_heroic_art_search_failures_are_warnings_without_writes() {
        for (answer, message) in [
            (Answer::Empty, "no SteamGridDB games match `Alpha`"),
            (Answer::Rejected, "SteamGridDB rejected the key"),
            (Answer::Offline, "offline"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let paths = paths(root.path());
            save_key(&paths);
            let access = access(&paths, answer, Rc::default());
            let choice = ArtChoice::checked((access.check_key)(), "Alpha");
            let before = snapshot::Snapshot::capture(root.path()).unwrap();
            let (mut out, mut err) = (Vec::new(), Vec::new());
            access.review(
                &choice.term,
                GlobalConfig::default().art.fetch,
                &mut out,
                &mut err,
            );
            before.assert_unchanged(root.path());
            assert!(out.is_empty());
            assert_eq!(
                String::from_utf8(err).unwrap(),
                format!("warning: art fetch failed: {message}\n")
            );
        }
    }

    #[test]
    fn dry_heroic_commands_with_art_only_search_and_never_apply() {
        for native in [false, true] {
            for all in [false, true] {
                for answer in [
                    Answer::Matches,
                    Answer::Empty,
                    Answer::Rejected,
                    Answer::Offline,
                ] {
                    let root = tempfile::tempdir().unwrap();
                    let paths = paths(root.path());
                    save_key(&paths);
                    let heroic = root.path().join("heroic");
                    fs::create_dir_all(heroic.join("sideload_apps")).unwrap();
                    fs::create_dir_all(heroic.join("GamesConfig")).unwrap();
                    let prefix = root.path().join("source");
                    fs::create_dir_all(prefix.join("drive_c")).unwrap();
                    fs::write(
                        prefix.join("system.reg"),
                        "WINE REGISTRY Version 2\n\n#arch=win64\n",
                    )
                    .unwrap();
                    let exe = prefix.join("drive_c/game");
                    let marker = root.path().join("runner-started");
                    write_script(
                        &exe,
                        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
                    );
                    let runner = paths.host_data().join("runners/test-wine/bin");
                    fs::create_dir_all(&runner).unwrap();
                    for program in ["wine", "wineserver"] {
                        let file = runner.join(program);
                        write_script(
                            &file,
                            format!(
                                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo wine-test; exit 0; fi\ntouch '{}'\nexit 1\n",
                                marker.display()
                            ),
                        );
                    }
                    let platform = if native { "linux" } else { "Windows" };
                    fs::write(heroic.join("sideload_apps/library.json"), format!(r#"{{"games":[{{"app_name":"a","title":"Alpha","runner":"sideload","install":{{"platform":"{platform}","executable":"{}"}}}}]}}"#, exe.display())).unwrap();
                    fs::write(heroic.join("config.json"), format!(r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#, prefix.display())).unwrap();
                    fs::write(heroic.join("GamesConfig/a.json"), r#"{"a":{}}"#).unwrap();
                    let calls = Rc::default();
                    let access = access(&paths, answer, Rc::clone(&calls));
                    let args = Args {
                        config_dir: Some(heroic.display().to_string()),
                        game: (!all).then(|| "a".to_owned()),
                        name: (!all).then(|| "alpha".to_owned()),
                        runner: (!native).then(|| "test-wine".to_owned()),
                        all,
                        art: true,
                        dry_run: true,
                        ..Args::default()
                    };
                    let before = snapshot::Snapshot::capture(root.path()).unwrap();
                    assert_eq!(
                        run_with_access(&paths, args, false, &access, true).unwrap(),
                        std::process::ExitCode::SUCCESS
                    );
                    before.assert_unchanged(root.path());
                    assert!(!marker.exists(), "runner must not start");
                    assert_eq!(
                        calls.borrow().as_slice(),
                        [format!("{}/search/autocomplete/Alpha", steamgriddb::API)]
                    );
                }
            }
        }
    }
}
