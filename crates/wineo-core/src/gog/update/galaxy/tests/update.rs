//! Galaxy updates against the fake content system: what's diffed, what's
//! reused, deleted, and saved aside, and how an interrupted one finishes.

use super::setup::{VC_EXE, scripted, with_setup};
use super::*;
use crate::gog::update::apply::{Options, Updated};
use crate::gog::update::galaxy::record::SetupStep;
use crate::gog::update::galaxy::update::{self, Note, Planned, Start};
use crate::operation::{Locks, Operation};

/// Build 200 of the game, against `Fixture::game`'s build 100: `bin/Game.exe`
/// and `Data/big.dat` change, keeping their first chunks; `Data/core.dat`
/// stays; `empty.txt` goes; `new/fresh.dat` is new.
fn game_200() -> FakeBuild {
    FakeBuild::new(GAME, "200").depot(
        FakeDepot::new(GAME)
            .file(
                FakeFile::new("bin\\Game.exe", b"MZ the game itself, version two")
                    .flag("executable"),
            )
            .file(FakeFile::new("Data\\core.dat", b"core data, several chunks long").with_md5())
            .file(FakeFile::new(
                "Data\\big.dat",
                b"many chunks and no whole-file md5 v2",
            ))
            .file(FakeFile::new("new\\fresh.dat", b"brand new"))
            .file(FakeFile::new(
                &format!("goggame-{GAME}.info"),
                info(GAME, "200").as_bytes(),
            ))
            .directory("saves")
            .link("Data\\current", "core.dat"),
    )
}

/// The compressed MD5 a chunk of `bytes` is asked for by.
fn chunk(bytes: &[u8]) -> String {
    fake::content::md5_hex(&fake::content::zlib(bytes))
}

impl Fixture {
    /// Build 100 installed, then `next` listed before it as the latest.
    fn installed_then(&self, next: FakeBuild) {
        self.content.add_build(Fixture::game("100"));
        self.install();
        self.gog.lock().unwrap().newest_first = true;
        self.content.add_build(next);
    }

    fn update_planned(&self) -> Result<Planned> {
        update::plan(
            &self.paths,
            &mut self.session(),
            "game",
            Options {
                refresh: true,
                ..Options::default()
            },
            &Token::new(),
        )
    }

    fn update_plan(&self) -> update::Plan {
        match self.update_planned().unwrap() {
            Planned::Update(plan) => plan,
            Planned::UpToDate { .. } => panic!("up to date"),
        }
    }

    fn apply_update(&self, plan: &update::Plan) -> Result<Updated> {
        update::apply(
            &self.paths,
            &mut self.session(),
            plan,
            &Token::new(),
            |_| {},
        )
    }

    fn update(&self) -> Updated {
        self.apply_update(&self.update_plan()).unwrap()
    }

    /// The compressed MD5 of every chunk asked for since `from` requests.
    fn chunks_since(&self, from: usize) -> Vec<String> {
        self.server
            .requests()
            .iter()
            .filter(|request| ContentSystem::cdn_of(request).is_some())
            .skip(from)
            .map(|request| {
                request
                    .path()
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    }

    fn chunk_count(&self) -> usize {
        self.chunk_requests().len()
    }

    fn base_build(&self) -> Option<String> {
        self.state().committed.unwrap().products[0].build.clone()
    }
}

/// Build 200 committed, with its files, the record's `installed` matching,
/// and nothing unfinished.
fn assert_updated(fx: &Fixture) {
    let state = fx.state();
    assert_eq!(state.journal, None);
    assert_eq!(fx.base_build().as_deref(), Some("200"));
    assert_eq!(fx.read("bin/Game.exe"), "MZ the game itself, version two");
    assert_eq!(fx.read("Data/core.dat"), "core data, several chunks long");
    assert_eq!(
        fx.read("Data/big.dat"),
        "many chunks and no whole-file md5 v2"
    );
    assert_eq!(fx.read("new/fresh.dat"), "brand new");
    assert_eq!(fx.read(&format!("goggame-{GAME}.info")), info(GAME, "200"));
    assert!(
        !fx.tree()
            .iter()
            .any(|path| path.contains(".wineo-") && !path.contains(".wineo-saved")),
        "{:?}",
        fx.tree()
    );
    let record = fx.record().unwrap();
    // Dropped after the commit, and ignored while there's no journal.
    assert!(record::pending(&state, &record).is_none());
    let installed = record::installed(&state, &record).unwrap().unwrap();
    assert_eq!(installed.manifest.build, "200");
    // What updates and repairs compare against later.
    assert!(
        installed
            .manifest
            .files
            .iter()
            .all(|file| file.md5.is_some()),
        "{:?}",
        installed.manifest.files
    );
    assert!(!fx.launch_refused());
}

#[test]
fn an_update_downloads_new_and_changed_files_reusing_chunks_the_old_copies_hold() {
    let fx = Fixture::new();
    fx.installed_then(game_200());
    let plan = fx.update_plan();
    assert_eq!(plan.start(), Start::Stored);
    assert_eq!(plan.installed_build(), Some("100"));
    assert_eq!(plan.build(), "200");
    assert!(!plan.is_recovery());
    assert!(plan.notes().is_empty(), "{:?}", plan.notes());
    let before = fx.chunk_count();
    let updated = fx.apply_update(&plan).unwrap();
    assert_updated(&fx);
    assert!(updated.saved.is_empty() && updated.kept.is_empty());
    // Old files the new build doesn't list are deleted.
    assert!(!fx.folder.join("empty.txt").exists());

    let asked = fx.chunks_since(before);
    // Only the chunks no old copy holds.
    for wanted in [
        b"lf, vers".as_slice(),
        b"ion two",
        b"5 v2",
        b"brand ne",
        b"w",
    ] {
        assert!(asked.contains(&chunk(wanted)), "{wanted:?}: {asked:?}");
    }
    for reused in [
        b"MZ the g".as_slice(),
        b"ame itse",
        b"many chu",
        b"nks and ",
        b"no whole",
        b"-file md",
        b"core dat",
    ] {
        assert!(!asked.contains(&chunk(reused)), "{reused:?}: {asked:?}");
    }
}

#[test]
fn a_damaged_old_copy_isnt_reused_and_your_edit_is_saved() {
    let fx = Fixture::new();
    fx.installed_then(game_200());
    // The first chunk's bytes changed, the rest still match.
    fs::write(
        fx.folder.join("Data/big.dat"),
        "MANY chunks and no whole-file md5",
    )
    .unwrap();
    let before = fx.chunk_count();
    let updated = fx.update();
    assert_updated(&fx);
    let asked = fx.chunks_since(before);
    assert!(asked.contains(&chunk(b"many chu")), "{asked:?}");
    assert!(!asked.contains(&chunk(b"nks and ")), "{asked:?}");
    // Your edit holds no hash wineo recorded, so it's kept aside.
    assert_eq!(
        fx.read("Data/big.dat.wineo-saved"),
        "MANY chunks and no whole-file md5"
    );
    assert_eq!(
        updated
            .saved
            .iter()
            .map(|saved| saved.path.as_str())
            .collect::<Vec<_>>(),
        ["Data/big.dat"]
    );
}

#[test]
fn a_deleted_file_you_changed_is_kept_and_listed() {
    let fx = Fixture::new();
    fx.installed_then(game_200());
    fs::write(fx.folder.join("empty.txt"), "mine").unwrap();
    let updated = fx.update();
    assert_updated(&fx);
    assert_eq!(fx.read("empty.txt"), "mine");
    assert_eq!(updated.kept, ["empty.txt"]);
}

#[test]
fn something_of_yours_at_a_new_path_is_moved_aside() {
    let fx = Fixture::new();
    fx.installed_then(game_200());
    fs::create_dir_all(fx.folder.join("new")).unwrap();
    fs::write(fx.folder.join("new/fresh.dat"), "mine").unwrap();
    let updated = fx.update();
    assert_updated(&fx);
    assert_eq!(fx.read("new/fresh.dat.wineo-saved"), "mine");
    assert_eq!(updated.saved.len(), 1);
    assert_eq!(updated.saved[0].path, "new/fresh.dat");
}

#[test]
fn a_file_of_yours_already_matching_the_new_build_is_kept_not_fetched() {
    let fx = Fixture::new();
    fx.installed_then(game_200());
    fs::create_dir_all(fx.folder.join("new")).unwrap();
    fs::write(fx.folder.join("new/fresh.dat"), "brand new").unwrap();
    let before = fx.chunk_count();
    let updated = fx.update();
    assert_updated(&fx);
    assert!(updated.saved.is_empty());
    assert!(!fx.chunks_since(before).contains(&chunk(b"brand ne")));
}

#[test]
fn a_file_whose_name_changes_case_is_updated_where_it_is() {
    let fx = Fixture::new();
    fx.installed_then(
        FakeBuild::new(GAME, "200").depot(
            FakeDepot::new(GAME)
                .file(FakeFile::new(
                    "BIN\\GAME.EXE",
                    b"MZ the game itself, version two",
                ))
                .file(FakeFile::new("data\\CORE.dat", b"core data, several chunks long").with_md5())
                .file(FakeFile::new(
                    &format!("goggame-{GAME}.info"),
                    info(GAME, "200").as_bytes(),
                )),
        ),
    );
    fx.update();
    assert_eq!(fx.read("bin/Game.exe"), "MZ the game itself, version two");
    assert_eq!(fx.read("Data/core.dat"), "core data, several chunks long");
    assert!(!fx.folder.join("BIN").exists());
    assert!(!fx.folder.join("data").exists());
    // Gone from the new build, and deleted.
    assert!(!fx.folder.join("Data/big.dat").exists());
    assert!(!fx.folder.join("saves").exists());
    assert!(fs::symlink_metadata(fx.folder.join("Data/current")).is_err());
}

/// A build with a folder of files, `Old/` holding `a.dat` and `Mods/`
/// holding `b.dat`, beside the info file.
fn with_folders(build: &str) -> FakeBuild {
    FakeBuild::new(GAME, build).depot(
        FakeDepot::new(GAME)
            .file(FakeFile::new("Old\\a.dat", b"old a"))
            .file(FakeFile::new("Mods\\b.dat", b"mods b"))
            .file(FakeFile::new("keep.dat", b"kept"))
            .file(FakeFile::new(
                &format!("goggame-{GAME}.info"),
                info(GAME, build).as_bytes(),
            )),
    )
}

#[test]
fn folders_emptied_by_deletions_go_and_setup_extras_stay() {
    let fx = Fixture::new();
    fx.content.add_build(with_folders("100"));
    fx.install();
    // What setup programs leave in a game folder, and a file of yours in
    // one of the game's folders.
    for extra in [
        "gog.ico",
        "support.ico",
        "goglog.ini",
        "unins000.exe",
        "unins000.dat",
        "Launch Fake Game.lnk",
    ] {
        fs::write(fx.folder.join(extra), extra).unwrap();
    }
    fs::write(fx.folder.join("Mods/mine.txt"), "mine").unwrap();
    fx.gog.lock().unwrap().newest_first = true;
    fx.content.add_build(
        FakeBuild::new(GAME, "200").depot(
            FakeDepot::new(GAME)
                .file(FakeFile::new("keep.dat", b"kept"))
                .file(FakeFile::new(
                    &format!("goggame-{GAME}.info"),
                    info(GAME, "200").as_bytes(),
                )),
        ),
    );
    let updated = fx.update();
    assert!(!fx.folder.join("Old").exists());
    assert!(!fx.folder.join("Mods/b.dat").exists());
    assert_eq!(fx.read("Mods/mine.txt"), "mine");
    for extra in [
        "gog.ico",
        "support.ico",
        "goglog.ini",
        "unins000.exe",
        "unins000.dat",
        "Launch Fake Game.lnk",
    ] {
        assert_eq!(fx.read(extra), extra);
    }
    assert!(updated.kept.is_empty() && updated.saved.is_empty());
}

#[test]
fn a_type_change_replaces_wineos_entry_and_saves_yours() {
    for edited in [false, true] {
        let fx = Fixture::new();
        fx.content.add_build(with_folders("100"));
        fx.install();
        if edited {
            fs::write(fx.folder.join("Old/a.dat"), "changed by you").unwrap();
        }
        fx.gog.lock().unwrap().newest_first = true;
        // `Old` becomes a file, and `keep.dat` a link.
        fx.content.add_build(
            FakeBuild::new(GAME, "200").depot(
                FakeDepot::new(GAME)
                    .file(FakeFile::new("Old", b"a file now"))
                    .link("keep.dat", "Old")
                    .file(FakeFile::new(
                        &format!("goggame-{GAME}.info"),
                        info(GAME, "200").as_bytes(),
                    )),
            ),
        );
        let updated = fx.update();
        assert_eq!(fx.read("Old"), "a file now", "edited {edited}");
        assert_eq!(
            fs::read_link(fx.folder.join("keep.dat")).unwrap(),
            Path::new("Old")
        );
        if edited {
            assert_eq!(fx.read("Old.wineo-saved/a.dat"), "changed by you");
            assert!(updated.saved.iter().any(|saved| saved.path == "Old"));
        } else {
            assert!(updated.saved.is_empty(), "{:?}", updated.saved);
            assert!(!fx.tree().iter().any(|path| path.contains("wineo-saved")));
        }
    }
}

#[test]
fn an_up_to_date_game_has_nothing_to_do() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    fx.install();
    match fx.update_planned().unwrap() {
        Planned::UpToDate { build, .. } => assert_eq!(build.as_deref(), Some("100")),
        Planned::Update(plan) => panic!("{plan:?}"),
    }
}

/// The game installed, its stored manifest no longer counting, as for a
/// game another tool installed.
fn forget_record(fx: &Fixture) {
    fs::remove_file(folder::sidecar_file(&fx.paths, &fx.folder)).unwrap();
}

#[test]
fn with_no_stored_manifest_the_installed_builds_is_fetched() {
    let fx = Fixture::new();
    fx.installed_then(game_200());
    forget_record(&fx);
    // Damage the size check can't see, and damage it can.
    fs::write(
        fx.folder.join("Data/core.dat"),
        "core data, several chunks LONG",
    )
    .unwrap();
    let plan = fx.update_plan();
    assert_eq!(plan.start(), Start::Fetched);
    let before = fx.chunk_count();
    let updated = fx.apply_update(&plan).unwrap();
    // Unchanged by the update and the right size: kept as it is, which a
    // repair would find.
    assert_eq!(fx.read("Data/core.dat"), "core data, several chunks LONG");
    // Old files still holding GOG's content are deleted.
    assert!(!fx.folder.join("empty.txt").exists());
    assert_eq!(
        fx.read("Data/big.dat"),
        "many chunks and no whole-file md5 v2"
    );
    assert!(updated.saved.is_empty(), "{:?}", updated.saved);
    // Chunks the old copies hold are reused here too.
    assert!(!fx.chunks_since(before).contains(&chunk(b"many chu")));

    // One of another size is downloaded again and yours saved aside.
    let fx = Fixture::new();
    fx.installed_then(game_200());
    forget_record(&fx);
    fs::write(fx.folder.join("Data/core.dat"), "shorter").unwrap();
    let updated = fx.update();
    assert_updated(&fx);
    assert_eq!(fx.read("Data/core.dat.wineo-saved"), "shorter");
    assert_eq!(updated.saved.len(), 1);
}

#[test]
fn with_no_manifest_to_be_had_nothing_is_deleted_and_mismatches_move_aside() {
    let fx = Fixture::new();
    fx.installed_then(game_200());
    forget_record(&fx);
    fx.hide("100");
    let plan = fx.update_plan();
    assert_eq!(plan.start(), Start::Target);
    assert_eq!(plan.notes(), [Note::NothingDeleted]);
    let updated = fx.apply_update(&plan).unwrap();
    assert_updated(&fx);
    // Files of the old build the new one doesn't list stay.
    assert!(fx.folder.join("empty.txt").is_file());
    // What doesn't match the new build is moved aside, what does is kept.
    let mut saved: Vec<&str> = updated
        .saved
        .iter()
        .map(|saved| saved.path.as_str())
        .collect();
    saved.sort_unstable();
    assert_eq!(
        saved,
        [
            "Data/big.dat",
            "bin/Game.exe",
            &format!("goggame-{GAME}.info")
        ]
    );
    assert_eq!(fx.read("bin/Game.exe.wineo-saved"), "MZ the game itself");
}

#[test]
fn a_build_adopted_from_another_tool_updates_from_its_fetched_manifest() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    fx.install();
    fx.gog.lock().unwrap().newest_first = true;
    fx.content.add_build(Fixture::game("150"));
    fx.content.add_build(game_200());
    // Another tool moved it to 150 and back to 100: the build is adopted
    // each time, and the stored manifest goes with the first.
    for build in ["150", "100"] {
        fs::write(
            fx.folder.join(format!("goggame-{GAME}.info")),
            info(GAME, build),
        )
        .unwrap();
        crate::gog::update::check(&mut fx.session(), "game", true).unwrap();
        assert_eq!(fx.base_build().as_deref(), Some(build));
    }
    assert!(fx.record().unwrap().installed.is_none());
    let plan = fx.update_plan();
    assert_eq!(plan.start(), Start::Fetched);
    assert_eq!(plan.installed_build(), Some("100"));
    fx.apply_update(&plan).unwrap();
    assert_updated(&fx);
    assert!(!fx.folder.join("empty.txt").exists());
}

/// Splits the folder's and the data directory's filesystems, each with
/// `free` bytes.
fn split_filesystems(fx: &Fixture, folder_free: u64, data_free: u64) {
    let data = fx.paths.gog_redist_dir().parent().unwrap().to_owned();
    DEVICE_AT.with(|list| {
        let mut list = list.borrow_mut();
        list.push((fx.folder.clone(), 1));
        list.push((data.clone(), 2));
    });
    FREE_AT.with(|list| {
        let mut list = list.borrow_mut();
        list.push((fx.folder.clone(), folder_free));
        list.push((data, data_free));
    });
}

fn clear_filesystems() {
    DEVICE_AT.with(|list| list.borrow_mut().clear());
    FREE_AT.with(|list| list.borrow_mut().clear());
}

#[test]
fn space_counts_staged_files_with_no_credit_for_deletions() {
    // Build 200 deletes a large file and adds smaller ones, plus a support
    // file in the data directory.
    let big = vec![b'x'; 4096];
    let next = || {
        FakeBuild::new(GAME, "200").depot(
            FakeDepot::new(GAME)
                .file(FakeFile::new("new.dat", &[b'n'; 300]))
                .file(FakeFile::new("setup.exe", &[b's'; 200]).flag("support"))
                .file(FakeFile::new(
                    &format!("goggame-{GAME}.info"),
                    info(GAME, "200").as_bytes(),
                )),
        )
    };
    let staged = 300 + info(GAME, "200").len() as u64;
    for (folder_free, data_free, refused) in [
        (staged - 1, 1 << 20, Some("Games")),
        (1 << 20, 199, Some("support")),
        (staged + 100, 300, None),
    ] {
        let fx = Fixture::new();
        fx.content.add_build(
            Fixture::game("100").depot(FakeDepot::new(GAME).file(FakeFile::new("huge.dat", &big))),
        );
        fx.install();
        fx.gog.lock().unwrap().newest_first = true;
        fx.content.add_build(next());
        let plan = fx.update_plan();
        let state = fx.state();
        let record = fs::read(folder::sidecar_file(&fx.paths, &fx.folder)).unwrap();
        split_filesystems(&fx, folder_free, data_free);
        let result = fx.apply_update(&plan);
        clear_filesystems();
        match refused {
            Some(which) => {
                let error = result.unwrap_err();
                assert!(error.to_string().starts_with("Not enough space"), "{error}");
                assert!(error.to_string().contains(which), "{error}");
                // Nothing changed.
                assert_eq!(fx.state(), state);
                assert_eq!(
                    fs::read(folder::sidecar_file(&fx.paths, &fx.folder)).unwrap(),
                    record
                );
                assert!(fx.folder.join("huge.dat").is_file());
            }
            None => {
                result.unwrap();
                assert!(!fx.folder.join("huge.dat").exists());
                assert_eq!(fx.base_build().as_deref(), Some("200"));
            }
        }
    }
}

#[test]
fn newly_owned_dlc_installs_with_the_update() {
    let fx = Fixture::new();
    fx.gog.lock().unwrap().dlc = vec![DLC];
    fx.gog.lock().unwrap().owned = Some(vec![GAME]);
    fx.content.add_build(Fixture::game("100"));
    fx.install();
    assert_eq!(fx.state().committed.unwrap().products.len(), 1);
    fx.gog.lock().unwrap().owned = Some(vec![GAME, DLC]);
    fx.gog.lock().unwrap().newest_first = true;
    fx.content.add_build(
        game_200().depot(FakeDepot::new(DLC).file(FakeFile::new("Data\\dlc.dat", b"bought"))),
    );
    let plan = fx.update_plan();
    assert_eq!(
        plan.products()
            .iter()
            .map(|product| (product.id, product.status))
            .collect::<Vec<_>>(),
        [
            (GAME, ProductStatus::Installed),
            (DLC, ProductStatus::Installed)
        ]
    );
    fx.apply_update(&plan).unwrap();
    assert_updated(&fx);
    assert_eq!(fx.read("Data/dlc.dat"), "bought");
    let products = fx.state().committed.unwrap().products;
    assert_eq!(products[1].id, DLC);
    assert_eq!(products[1].build.as_deref(), Some("200"));
}

#[test]
fn an_unavailable_dlc_is_retried_on_the_same_build() {
    let fx = Fixture::new();
    fx.gog.lock().unwrap().dlc = vec![DLC];
    // Build 100 has no files for the DLC at first.
    fx.content.add_build(Fixture::game("100"));
    fx.install();
    let products = fx.state().committed.unwrap().products;
    assert_eq!(products[1].status, ProductStatus::Unavailable);
    // Nothing to do while the build still lacks it.
    assert!(matches!(
        fx.update_planned().unwrap(),
        Planned::UpToDate { .. }
    ));
    // Now listed with it, under the same build id.
    fx.gog.lock().unwrap().newest_first = true;
    fx.content.add_build(
        Fixture::game("100").depot(FakeDepot::new(DLC).file(FakeFile::new("dlc.dat", b"now"))),
    );
    let plan = fx.update_plan();
    assert_eq!(plan.installed_build(), Some("100"));
    assert_eq!(plan.build(), "100");
    let before = fx.chunk_count();
    fx.apply_update(&plan).unwrap();
    assert_eq!(fx.read("dlc.dat"), "now");
    let products = fx.state().committed.unwrap().products;
    assert_eq!(products[1].status, ProductStatus::Installed);
    // The game's own files were already in place.
    assert_eq!(fx.chunks_since(before), [chunk(b"now")]);
}

#[test]
fn an_update_interrupted_at_each_step_finishes_without_downloading_again() {
    for (point, skip) in [
        ("galaxy.update.before", 0),
        ("galaxy.update.pending.after", 0),
        ("galaxy.update.journal.after", 0),
        ("galaxy.update.placed.after", 0),
        ("galaxy.update.support.after", 0),
        ("galaxy.update.file.after", 0),
        ("galaxy.update.file.after", 2),
        ("galaxy.named.after", 0),
        ("galaxy.aside.after", 0),
        ("galaxy.update.files.after", 0),
        ("update.delete.after", 0),
        ("galaxy.update.deleted.after", 0),
        ("galaxy.update.setup.after", 0),
        ("galaxy.update.installed.after", 0),
        ("galaxy.update.commit.after", 0),
    ] {
        let fx = Fixture::new();
        fx.installed_then(game_200());
        let plan = fx.update_plan();
        set_failpoint_nth(point, skip);
        let error = fx.apply_update(&plan).unwrap_err();
        set_failpoint(None);
        assert!(
            format!("{error:#}").contains("injected"),
            "{point}: {error:#}"
        );
        let state = fx.state();
        let journaled = state.journal.is_some();
        assert_eq!(fx.launch_refused(), journaled, "{point}");
        if !journaled {
            // Nothing had started, or it was committed.
            let build = fx.base_build();
            assert!(
                matches!(
                    point,
                    "galaxy.update.before"
                        | "galaxy.update.pending.after"
                        | "galaxy.update.commit.after"
                ),
                "{point}: {build:?}"
            );
            if point == "galaxy.update.commit.after" {
                assert_updated(&fx);
                continue;
            }
        }
        // Files already whole in place are never downloaded again.
        let whole: Vec<String> = TARGET_200
            .iter()
            .filter(|(path, bytes)| fs::read(fx.folder.join(path)).ok().as_deref() == Some(*bytes))
            .flat_map(|(_, bytes)| bytes.chunks(8).map(chunk))
            .collect();
        let before = fx.chunk_count();
        let plan = fx.update_plan();
        assert_eq!(plan.is_recovery(), journaled, "{point}");
        fx.apply_update(&plan).unwrap();
        assert_updated(&fx);
        let asked = fx.chunks_since(before);
        for hash in &whole {
            assert!(!asked.contains(hash), "{point}: {hash} asked for again");
        }
        assert!(!fx.folder.join("empty.txt").exists(), "{point}");
    }
}

/// Build 200's files whose chunks no other file of it shares.
const TARGET_200: [(&str, &[u8]); 4] = [
    ("bin/Game.exe", b"MZ the game itself, version two"),
    ("Data/big.dat", b"many chunks and no whole-file md5 v2"),
    ("new/fresh.dat", b"brand new"),
    ("Data/core.dat", b"core data, several chunks long"),
];

/// Build 300: build 200 with its own info file and one more file.
fn game_300() -> FakeBuild {
    let mut build = game_200();
    build.build_id = "300".to_owned();
    build.version_name = "v300".to_owned();
    build.depot(
        FakeDepot::new(GAME)
            .file(FakeFile::new(
                &format!("goggame-{GAME}.info"),
                info(GAME, "300").as_bytes(),
            ))
            .file(FakeFile::new("three.dat", b"three")),
    )
}

/// An update to build 200 stopped by an injected failure at `point`.
fn update_interrupted(fx: &Fixture, point: &'static str, skip: usize) {
    let plan = fx.update_plan();
    set_failpoint_nth(point, skip);
    let result = fx.apply_update(&plan);
    set_failpoint(None);
    let error = result.unwrap_err();
    assert!(format!("{error:#}").contains("injected"), "{error:#}");
}

#[test]
fn without_pending_an_update_fetches_its_builds_again() {
    let fx = Fixture::new();
    fx.installed_then(game_200());
    update_interrupted(&fx, "galaxy.update.file.after", 1);
    forget_record(&fx);
    let plan = fx.update_plan();
    assert!(plan.is_recovery());
    assert!(plan.retarget().is_none());
    assert_eq!(plan.build(), "200");
    // The installed build is still listed, so it's the starting point and
    // old files are deleted on a hash match.
    assert_eq!(plan.start(), Start::Fetched);
    let updated = fx.apply_update(&plan).unwrap();
    assert_updated(&fx);
    assert!(updated.recovered);
    assert!(updated.saved.is_empty(), "{:?}", updated.saved);
    assert!(!fx.folder.join("empty.txt").exists());
}

#[test]
fn without_pending_an_update_whose_build_is_gone_finishes_with_the_latest() {
    let fx = Fixture::new();
    fx.installed_then(game_200());
    update_interrupted(&fx, "galaxy.update.files.after", 0);
    fs::write(folder::sidecar_file(&fx.paths, &fx.folder), b"garbage").unwrap();
    fx.hide("200");
    fx.content.add_build(game_300());
    let plan = fx.update_plan();
    assert_eq!(
        plan.retarget(),
        Some(&install::Retarget {
            from: "200".to_owned(),
            to: "300".to_owned()
        })
    );
    assert_eq!(plan.start(), Start::Target);
    assert_eq!(plan.notes(), [Note::NothingDeleted]);
    let before = fx.chunk_count();
    let updated = fx.apply_update(&plan).unwrap();
    assert_eq!(fx.base_build().as_deref(), Some("300"));
    // What matches build 300 stays, the info file the update wrote is its
    // own to replace, and nothing is deleted or moved aside.
    assert!(updated.saved.is_empty(), "{:?}", updated.saved);
    assert!(fx.folder.join("empty.txt").is_file());
    assert_eq!(fx.read(&format!("goggame-{GAME}.info")), info(GAME, "300"));
    assert_eq!(fx.read("three.dat"), "three");
    let asked = fx.chunks_since(before);
    assert!(!asked.contains(&chunk(b"brand ne")), "{asked:?}");
}

#[test]
fn an_unstarted_updates_pending_never_makes_a_file_tracked() {
    let fx = Fixture::new();
    fx.installed_then(game_200());
    // Stopped after pending, before the journal.
    update_interrupted(&fx, "galaxy.update.pending.after", 0);
    assert!(fx.state().journal.is_none());
    // The stored manifest is lost, and the unstarted update's pending
    // claims a file of yours at exactly its content.
    fs::write(fx.folder.join("bin/Game.exe"), "yours").unwrap();
    let mut record = fx.record().unwrap();
    record.installed = None;
    let pending = record.pending.as_mut().unwrap();
    let base = pending.base.as_mut().unwrap();
    for file in &mut base.manifest.files {
        if file.path == "bin/Game.exe" {
            file.md5 = Some(fake::content::md5_hex(b"yours"));
        }
    }
    write_record(&fx, &record);
    fx.hide("100");
    let plan = fx.update_plan();
    assert!(!plan.is_recovery());
    assert_eq!(plan.start(), Start::Target);
    let updated = fx.apply_update(&plan).unwrap();
    assert_updated(&fx);
    assert_eq!(fx.read("bin/Game.exe.wineo-saved"), "yours");
    assert!(
        updated
            .saved
            .iter()
            .any(|saved| saved.path == "bin/Game.exe")
    );
}

#[test]
fn a_dlc_only_update_interrupted_anywhere_finishes_once() {
    for point in [
        "galaxy.update.pending.after",
        "galaxy.update.files.after",
        "galaxy.update.commit.after",
    ] {
        let fx = Fixture::new();
        fx.gog.lock().unwrap().dlc = vec![DLC];
        fx.gog.lock().unwrap().owned = Some(vec![GAME]);
        fx.content.add_build(
            Fixture::game("100")
                .depot(FakeDepot::new(DLC).file(FakeFile::new("dlc.dat", b"bought"))),
        );
        fx.install();
        assert!(!fx.folder.join("dlc.dat").exists());
        fx.gog.lock().unwrap().owned = Some(vec![GAME, DLC]);
        let plan = fx.update_plan();
        assert_eq!(plan.build(), "100");
        set_failpoint(Some(point));
        let error = fx.apply_update(&plan).unwrap_err();
        set_failpoint(None);
        assert!(
            format!("{error:#}").contains("injected"),
            "{point}: {error:#}"
        );
        if point == "galaxy.update.commit.after" {
            assert!(
                matches!(fx.update_planned().unwrap(), Planned::UpToDate { .. }),
                "{point}"
            );
        } else {
            let plan = fx.update_plan();
            assert_eq!(
                plan.is_recovery(),
                point != "galaxy.update.pending.after",
                "{point}"
            );
            // The committed selection's record still counts, or pending
            // carries on.
            assert_eq!(plan.start(), Start::Stored, "{point}");
            fx.apply_update(&plan).unwrap();
        }
        assert_eq!(fx.read("dlc.dat"), "bought", "{point}");
        let state = fx.state();
        assert!(state.journal.is_none());
        let products = state.committed.as_ref().unwrap().products.clone();
        assert_eq!(products[1].status, ProductStatus::Installed, "{point}");
        let record = fx.record().unwrap();
        assert!(record::installed(&state, &record).unwrap().is_some());
        assert!(!fx.tree().iter().any(|path| path.contains("wineo-saved")));
    }
}

#[test]
fn setup_reruns_only_the_steps_whose_identity_changed() {
    let fx = Fixture::new();
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, None);
    fx.install();
    assert_eq!(fx.names(), ["scriptinterpreter.exe", "VC_redist.x86.exe"]);
    fx.gog.lock().unwrap().newest_first = true;
    // A new build, with the same runtime: the script reruns, the runtime
    // doesn't.
    fx.content.add_build(scripted("200"));
    fx.update();
    assert_eq!(
        fx.names(),
        [
            "scriptinterpreter.exe",
            "VC_redist.x86.exe",
            "scriptinterpreter.exe"
        ]
    );
    // A new build with a new version of the runtime: both.
    fx.content.add_build(scripted("300"));
    with_setup(&fx, None, Some("0"));
    fx.update();
    assert_eq!(fx.names().len(), 5);
    assert_eq!(
        &fx.names()[3..],
        ["scriptinterpreter.exe", "VC_redist.x86.exe"]
    );
    let record = fx.record().unwrap();
    assert!(matches!(
        &record.setup[..],
        [
            SetupStep::ScriptInterpreter { build, .. },
            SetupStep::Redistributable { .. }
        ] if build == "300"
    ));
}

#[test]
fn an_update_stopped_in_setup_carries_on_from_the_step_without_downloading() {
    for (point, skip) in [
        ("galaxy.setup.ran", 0),
        ("galaxy.setup.ran", 1),
        ("galaxy.update.setup.after", 0),
    ] {
        let fx = Fixture::new();
        fx.content.add_build(scripted("100"));
        with_setup(&fx, None, None);
        fx.install();
        fx.gog.lock().unwrap().newest_first = true;
        fx.content.add_build(scripted("200"));
        with_setup(&fx, None, Some("0"));
        update_interrupted(&fx, point, skip);
        assert!(fx.launch_refused(), "{point} {skip}");
        let ran = fx.names().len();
        let before = fx.chunk_count();
        let plan = fx.update_plan();
        assert!(plan.is_recovery());
        fx.apply_update(&plan).unwrap();
        assert_eq!(fx.chunk_count(), before, "{point} {skip}");
        // The step that hadn't been recorded runs again, and the rest only
        // if they hadn't run.
        let again = fx.names()[ran..].to_vec();
        let expected: &[&str] = match (point, skip) {
            ("galaxy.setup.ran", 0) => &["scriptinterpreter.exe", "VC_redist.x86.exe"],
            ("galaxy.setup.ran", _) => &["VC_redist.x86.exe"],
            _ => &[],
        };
        assert_eq!(again, expected, "{point} {skip}");
        assert!(!fx.launch_refused());
        assert_eq!(fx.base_build().as_deref(), Some("200"));
    }
}

#[test]
fn a_failed_setup_step_blocks_launch_until_updating_again() {
    let fx = Fixture::new();
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, None);
    fx.install();
    fx.gog.lock().unwrap().newest_first = true;
    fx.content.add_build(scripted("200"));
    with_setup(&fx, None, Some("5"));
    let error = fx.apply_update(&fx.update_plan()).unwrap_err();
    assert!(
        matches!(error.downcast_ref(), Some(GalaxyError::Setup { .. })),
        "{error:#}"
    );
    assert!(fx.launch_refused());
    fx.exits("MSVC2017", VC_EXE, "0");
    let before = fx.chunk_count();
    fx.update();
    assert_eq!(fx.chunk_count(), before);
    assert!(!fx.launch_refused());
}

#[test]
fn an_update_refuses_while_the_game_runs() {
    let fx = Fixture::new();
    fx.installed_then(game_200());
    let plan = fx.update_plan();
    let launch = Locks::new(&fx.paths)
        .acquire(Operation::Launch, "game", &[])
        .unwrap();
    let error = fx.apply_update(&plan).unwrap_err();
    assert!(
        matches!(
            error.downcast_ref(),
            Some(crate::gog::update::UpdateError::Running { .. })
        ),
        "{error:#}"
    );
    drop(launch);
    assert_eq!(fx.base_build().as_deref(), Some("100"));
    assert!(fx.state().journal.is_none());
    fx.apply_update(&plan).unwrap();
    assert_updated(&fx);
}

#[test]
fn a_snapshot_restore_during_an_updates_setup_is_refused() {
    let fx = Fixture::new();
    fx.content.add_build(scripted("100"));
    with_setup(&fx, None, None);
    fx.install();
    fx.gog.lock().unwrap().newest_first = true;
    fx.content.add_build(scripted("200"));
    let prefix = fx.prefix();
    let refused = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = std::rc::Rc::clone(&refused);
    super::setup::set_before_setup_step(move |paths, _| {
        seen.borrow_mut().push(
            Locks::new(paths)
                .acquire(Operation::Snapshot, "game", &[&prefix])
                .is_err(),
        );
    });
    fx.update();
    assert_eq!(*refused.borrow(), [true]);
}

#[test]
fn updates_reach_galaxy_games_through_core_but_not_yet_the_cli_path() {
    let fx = Fixture::new();
    fx.installed_then(game_200());
    let planned = crate::gog::update::galaxy::plan_update(
        &fx.paths,
        Some(&mut fx.session()),
        "game",
        Options::default(),
        &Token::new(),
    )
    .unwrap();
    assert!(matches!(
        planned,
        crate::gog::update::galaxy::PlannedUpdate::Galaxy(Planned::Update(_))
    ));
    // What the CLI and the app call still refuses, until they show Galaxy
    // updates.
    let error = crate::gog::update::plan(
        &fx.paths,
        Some(&mut fx.session()),
        "game",
        Options::default(),
        &Token::new(),
    )
    .unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(crate::gog::update::UpdateError::Galaxy { .. })
    ));
    // Checking still compares by build.
    let status = crate::gog::update::check(&mut fx.session(), "game", true).unwrap();
    assert_eq!(status.state, crate::gog::update::UpdateState::OnGog);
    assert_eq!(status.available.as_deref(), Some("200"));
    // A reinstall comes with repairs.
    let error = update::plan(
        &fx.paths,
        &mut fx.session(),
        "game",
        Options {
            reinstall: true,
            ..Options::default()
        },
        &Token::new(),
    )
    .unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(GalaxyError::Reinstall { .. })
    ));
}

#[test]
fn an_unfinished_galaxy_install_is_finished_by_installing_not_updating() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    interrupted(&fx, "galaxy.files.after", 0);
    let error = fx.update_planned().unwrap_err();
    assert!(
        matches!(
            error.downcast_ref(),
            Some(folder::FolderError::UnfinishedInstall { .. })
        ),
        "{error:#}"
    );
}

/// `build` with every depot for English alone.
fn in_english(mut build: FakeBuild) -> FakeBuild {
    build.depots = build
        .depots
        .into_iter()
        .map(|depot| depot.languages(&["en-US"]))
        .collect();
    build
}

#[test]
fn a_language_another_tool_recorded_finds_gogs_depots() {
    let fx = Fixture::new();
    fx.content.add_build(in_english(Fixture::game("100")));
    fx.install();
    // As Heroic may record it.
    let lock = FolderLock::exclusive(&fx.paths, &fx.folder, &|| false).unwrap();
    let mut state = lock.read().unwrap().unwrap();
    state.committed.as_mut().unwrap().language = "en".to_owned();
    lock.write(&state).unwrap();
    drop(lock);
    // On the latest build, that alone is nothing to update.
    assert!(matches!(
        fx.update_planned().unwrap(),
        Planned::UpToDate { .. }
    ));
    fx.gog.lock().unwrap().newest_first = true;
    fx.content.add_build(in_english(game_200()));
    let plan = fx.update_plan();
    assert_eq!(plan.start(), Start::Fetched);
    assert!(plan.warnings().is_empty(), "{:?}", plan.warnings());
    fx.apply_update(&plan).unwrap();
    assert_updated(&fx);
    assert!(!fx.folder.join("empty.txt").exists());
    let committed = fx.state().committed.unwrap();
    assert_eq!(committed.language, "en");
    assert_eq!(committed.products[0].language.as_deref(), Some("en-US"));
    assert!(matches!(
        fx.update_planned().unwrap(),
        Planned::UpToDate { .. }
    ));
}

#[test]
fn a_kept_file_with_no_known_md5_is_still_deleted_later_on_a_chunk_match() {
    let fx = Fixture::new();
    // Build 200 only changes the info file, so with no stored manifest
    // `Data/big.dat` is kept on its size, and nothing gives its MD5.
    fx.installed_then(Fixture::game("200"));
    forget_record(&fx);
    let plan = fx.update_plan();
    assert_eq!(plan.start(), Start::Fetched);
    fx.apply_update(&plan).unwrap();
    let record = fx.record().unwrap();
    let installed = record::installed(&fx.state(), &record).unwrap().unwrap();
    let big = installed
        .manifest
        .files
        .iter()
        .find(|file| file.path == "Data/big.dat")
        .unwrap();
    assert_eq!(big.md5, None);
    // Build 300 drops it: its chunks show it's GOG's, so it goes.
    fx.content
        .add_build(
            FakeBuild::new(GAME, "300").depot(FakeDepot::new(GAME).file(FakeFile::new(
                &format!("goggame-{GAME}.info"),
                info(GAME, "300").as_bytes(),
            ))),
        );
    let plan = fx.update_plan();
    assert_eq!(plan.start(), Start::Stored);
    let updated = fx.apply_update(&plan).unwrap();
    assert!(!fx.folder.join("Data/big.dat").exists());
    assert!(updated.kept.is_empty(), "{:?}", updated.kept);
}
