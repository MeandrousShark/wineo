//! Branches, pins, and the listing against the fake content system.

use super::update::game_200;
use super::*;
use crate::gog::update::apply::Options;
use crate::gog::update::galaxy::follow::Choice;
use crate::gog::update::galaxy::listing;
use crate::gog::update::galaxy::record::Follow;
use crate::gog::update::galaxy::update::{self, Planned};
use crate::gog::update::galaxy::{GalaxyError, switch};
use crate::gog::update::{Reason, UpdateError, UpdateState};

fn choosing(choice: Choice) -> Options {
    Options {
        choice,
        refresh: true,
        ..Options::default()
    }
}

impl Fixture {
    fn plan_choosing(&self, choice: Choice) -> Result<Planned> {
        update::plan(
            &self.paths,
            &mut self.session(),
            "game",
            choosing(choice),
            &Token::new(),
        )
    }

    /// Build 100 installed, with 200 ahead of it on the default branch and
    /// 300 on the beta branch.
    fn on_branches() -> Self {
        let fx = Self::new();
        fx.content.add_build(Fixture::game("100"));
        fx.install();
        fx.gog.lock().unwrap().newest_first = true;
        fx.content.add_build(game_200());
        fx.content.add_build(Fixture::game("300").branch("beta"));
        fx
    }

    fn follows(&self) -> Follow {
        self.record().unwrap().follow
    }

    fn pin(&self, build: &str) {
        let Planned::Follow(change) = self.plan_choosing(Choice::Build(build.to_owned())).unwrap()
        else {
            panic!("expected a change of pin");
        };
        switch::apply(&self.paths, &change, &Token::new()).unwrap();
    }
}

fn beta() -> Follow {
    Follow {
        branch: Some("beta".to_owned()),
        pinned: None,
    }
}

#[test]
fn a_choice_that_leaves_the_files_alone_only_writes_the_follow() {
    let fx = Fixture::on_branches();
    let before = fx.tree();
    let installed = fx.record().unwrap().installed;

    let Planned::Follow(change) = fx.plan_choosing(Choice::Build("100".to_owned())).unwrap() else {
        panic!("expected a change of pin");
    };
    assert_eq!(change.installed_build(), Some("100"));
    assert_eq!(change.from(), &Follow::default());
    assert_eq!(change.to().pinned.as_deref(), Some("100"));
    let updated = switch::apply(&fx.paths, &change, &Token::new()).unwrap();
    assert!(!updated.claimed);

    let record = fx.record().unwrap();
    assert_eq!(record.follow.pinned.as_deref(), Some("100"));
    // The stored manifest and the files are as they were, with no journal.
    assert_eq!(record.installed, installed);
    assert_eq!(record.pending, None);
    assert_eq!(fx.tree(), before);
    assert!(fx.state().journal.is_none());

    // Asking for the same again is nothing.
    assert!(matches!(
        fx.plan_choosing(Choice::Build("100".to_owned())).unwrap(),
        Planned::UpToDate { .. }
    ));
}

#[test]
fn a_change_planned_before_the_follow_changed_is_refused() {
    let fx = Fixture::on_branches();
    let Planned::Follow(change) = fx.plan_choosing(Choice::Build("100".to_owned())).unwrap() else {
        panic!("expected a change of pin");
    };
    // Another wineo pins another build first.
    fx.pin("100");
    let mut record = fx.record().unwrap();
    record.follow = beta();
    fs::write(
        folder::sidecar_file(&fx.paths, &fx.folder),
        record.to_json().unwrap(),
    )
    .unwrap();
    let error = switch::apply(&fx.paths, &change, &Token::new()).unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(UpdateError::Changed { .. })
    ));
    assert_eq!(fx.follows(), beta());
}

#[test]
fn an_update_commits_the_follow_it_was_planned_with_and_a_repair_keeps_it() {
    let fx = Fixture::on_branches();
    let Planned::Update(plan) = fx.plan_choosing(Choice::Branch("beta".to_owned())).unwrap() else {
        panic!("expected an update");
    };
    assert_eq!(plan.from(), &Follow::default());
    assert_eq!(plan.to(), &beta());
    assert_eq!(plan.build(), "300");
    // Nothing is followed until the files are in place.
    assert_eq!(fx.follows(), Follow::default());
    fx.apply_update(&plan).unwrap();
    assert_eq!(fx.follows(), beta());
    assert_eq!(fx.record().unwrap().pending, None);

    // A repair never changes the build or what is followed.
    let repair = Options {
        repair: true,
        ..Options::default()
    };
    let Planned::Update(plan) =
        update::plan(&fx.paths, &mut fx.session(), "game", repair, &Token::new()).unwrap()
    else {
        panic!("expected a repair");
    };
    assert_eq!(plan.from(), plan.to());
    fx.apply_update(&plan).unwrap();
    assert_eq!(fx.follows(), beta());
    assert_eq!(
        fx.state().committed.unwrap().products[0].build.as_deref(),
        Some("300")
    );
}

#[test]
fn a_pinned_game_does_not_update_but_a_reinstall_rebuilds_the_pin() {
    let fx = Fixture::on_branches();
    fx.pin("100");
    let Planned::Pinned(pinned) = fx.plan_choosing(Choice::Keep).unwrap() else {
        panic!("expected a pinned game");
    };
    assert_eq!(pinned.build, "100");
    assert_eq!(pinned.version.as_deref(), Some("v100"));
    assert_eq!(
        pinned.message(),
        "game is pinned to build 100 (v100). Run wineo update game --latest to follow its branch again."
    );

    let reinstall = Options {
        reinstall: true,
        refresh: true,
        ..Options::default()
    };
    let Planned::Update(plan) = update::plan(
        &fx.paths,
        &mut fx.session(),
        "game",
        reinstall,
        &Token::new(),
    )
    .unwrap() else {
        panic!("expected a reinstall");
    };
    assert_eq!(plan.build(), "100");
    assert_eq!(plan.from(), plan.to());

    let Planned::Update(plan) = fx.plan_choosing(Choice::Latest).unwrap() else {
        panic!("expected an update");
    };
    assert_eq!(plan.build(), "200");
    assert_eq!(plan.to(), &Follow::default());
}

#[test]
fn a_choice_waits_for_an_unfinished_operation() {
    let fx = Fixture::on_branches();
    let Planned::Update(plan) = fx.plan_choosing(Choice::Keep).unwrap() else {
        panic!("expected an update");
    };
    crate::gog::folder::set_failpoint(Some("galaxy.update.placed.after"));
    fx.apply_update(&plan).unwrap_err();
    crate::gog::folder::set_failpoint(None);
    assert!(fx.state().journal.is_some());
    for choice in [
        Choice::Branch("beta".to_owned()),
        Choice::DefaultBranch,
        Choice::Build("100".to_owned()),
        Choice::Latest,
    ] {
        let error = fx.plan_choosing(choice).unwrap_err();
        assert!(
            matches!(error.downcast_ref(), Some(GalaxyError::Unfinished { .. })),
            "{error:#}"
        );
    }
}

#[test]
fn a_check_follows_the_branch_and_holds_back_a_pinned_game() {
    let fx = Fixture::on_branches();
    let check = |fx: &Fixture| crate::gog::update::check(&mut fx.session(), "game", true).unwrap();
    let status = check(&fx);
    assert_eq!(status.available.as_deref(), Some("200"));
    assert_eq!(status.branch, None);
    assert!(status.applicable);

    fx.pin("100");
    let status = check(&fx);
    assert_eq!(status.state, UpdateState::Available);
    assert!(!status.applicable);
    assert_eq!(status.pinned.as_deref(), Some("100"));
    assert_eq!(status.pinned_name.as_deref(), Some("v100"));
    assert_eq!(
        status.reason,
        Some(Reason::Pinned {
            name: "game".to_owned(),
            build: "100".to_owned(),
            version: Some("v100".to_owned()),
        })
    );
    assert_eq!(status.reason.as_ref().unwrap().as_str(), "pinned");

    // Followed to a branch with nothing newer, a pin is still told.
    let Planned::Update(plan) = fx.plan_choosing(Choice::Branch("beta".to_owned())).unwrap() else {
        panic!("expected an update");
    };
    fx.apply_update(&plan).unwrap();
    let status = check(&fx);
    assert_eq!(status.state, UpdateState::UpToDate);
    assert_eq!(status.branch.as_deref(), Some("beta"));
    assert_eq!(status.pinned, None);
    assert_eq!(status.reason, None);
    assert_eq!(status.pin_note(), None);

    // A branch GOG lists nothing generation 2 on.
    fx.hide("300");
    let status = check(&fx);
    assert_eq!(status.state, UpdateState::OnGog);
    assert_eq!(
        status.reason,
        Some(Reason::Galaxy {
            name: "game".to_owned(),
            branch: Some("beta".to_owned()),
        })
    );
}

#[test]
fn the_listing_has_the_default_branch_first_and_only_generation_2_builds() {
    let fx = Fixture::on_branches();
    let mut old = Fixture::game("50");
    old.generation = 1;
    fx.content.add_build(old);
    fx.content.set_private_branches(true);
    fx.pin("100");

    let listed = listing::list(&fx.paths, &mut fx.session(), "game", None, true, false).unwrap();
    assert!(listed.private_branches);
    assert_eq!(listed.installed.as_deref(), Some("100"));
    assert_eq!(listed.follow.pinned.as_deref(), Some("100"));
    let seen: Vec<(Option<&str>, &str, Vec<&str>)> = listed
        .builds
        .iter()
        .map(|build| (build.branch.as_deref(), build.build.as_str(), build.marks()))
        .collect();
    assert_eq!(
        seen,
        [
            (None, "200", vec!["latest"]),
            (None, "100", vec!["installed", "pinned"]),
            (Some("beta"), "300", vec!["latest"]),
        ]
    );
}

#[test]
fn the_listing_of_an_installer_game_is_refused() {
    let fx = Fixture::on_branches();
    let mut state = fx.state();
    state.committed.as_mut().unwrap().track = Track::Installer;
    let lock = folder::FolderLock::exclusive(&fx.paths, &fx.folder, &|| false).unwrap();
    lock.write(&state).unwrap();
    drop(lock);
    let error = listing::list(&fx.paths, &mut fx.session(), "game", None, true, false).unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(UpdateError::NotGalaxy { .. })
    ));
}

#[test]
fn an_operation_planned_before_a_pin_is_refused_and_leaves_the_pin() {
    let fx = Fixture::on_branches();
    let Planned::Update(stale) = fx.plan_choosing(Choice::Keep).unwrap() else {
        panic!("expected an update");
    };
    let repair = Options {
        repair: true,
        ..Options::default()
    };
    let Planned::Update(stale_repair) =
        update::plan(&fx.paths, &mut fx.session(), "game", repair, &Token::new()).unwrap()
    else {
        panic!("expected a repair");
    };
    let before = fx.tree();
    // Another wineo pins the build installed, which touches only the record.
    fx.pin("100");
    for plan in [&stale, &stale_repair] {
        let error = fx.apply_update(plan).unwrap_err();
        assert!(
            matches!(error.downcast_ref(), Some(UpdateError::Changed { .. })),
            "{error:#}"
        );
    }
    assert_eq!(fx.follows().pinned.as_deref(), Some("100"));
    assert_eq!(fx.tree(), before);
    assert!(fx.state().journal.is_none());
    assert_eq!(
        fx.state().committed.unwrap().products[0].build.as_deref(),
        Some("100")
    );
}

impl Fixture {
    /// An install of the game's beta branch (or the build `choice` names)
    /// that stopped once its record was written, with `pending` lost.
    fn install_stopped_without_pending(&self, choice: Choice) {
        self.content.add_build(Fixture::game("100"));
        self.content.add_build(Fixture::game("300").branch("beta"));
        let plan = self
            .plan_with(&Request {
                builds: choice,
                ..self.request()
            })
            .unwrap();
        crate::gog::folder::set_failpoint(Some("galaxy.installed.after"));
        self.apply(&plan).unwrap_err();
        crate::gog::folder::set_failpoint(None);
        assert!(self.state().journal.is_some());
        let path = folder::sidecar_file(&self.paths, &self.folder);
        let mut saved: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(saved.as_object_mut().unwrap().remove("pending").is_some());
        fs::write(&path, saved.to_string()).unwrap();
    }
}

#[test]
fn an_install_whose_pending_is_lost_finishes_with_the_follow_its_record_kept() {
    let fx = Fixture::new();
    fx.install_stopped_without_pending(Choice::Branch("beta".to_owned()));
    assert_eq!(fx.follows(), beta());
    let plan = fx.plan_with(&fx.request()).unwrap();
    assert_eq!(plan.build(), "300");
    assert_eq!(plan.follow(), &beta());
    fx.apply(&plan).unwrap();
    assert_eq!(fx.follows(), beta());
    assert_eq!(fx.read("bin/Game.exe"), "MZ the game itself");
}

#[test]
fn an_install_retargets_to_the_branch_its_record_kept_and_drops_a_pin() {
    let fx = Fixture::new();
    fx.install_stopped_without_pending(Choice::Build("300".to_owned()));
    assert_eq!(fx.follows().pinned.as_deref(), Some("300"));
    fx.hide("300");
    fx.content.add_build(Fixture::game("301").branch("beta"));
    let plan = fx.plan_with(&fx.request()).unwrap();
    assert_eq!(plan.build(), "301");
    assert_eq!(plan.follow(), &beta());
    assert!(
        plan.warnings()
            .iter()
            .any(|warning| warning.ends_with("The pin to build 300 is removed.")),
        "{:?}",
        plan.warnings()
    );
    fx.apply(&plan).unwrap();
    assert_eq!(fx.follows(), beta());

    // A branch with nothing left says so instead of falling to the default.
    let fx = Fixture::new();
    fx.install_stopped_without_pending(Choice::Branch("beta".to_owned()));
    fx.hide("300");
    let error = fx.plan_with(&fx.request()).unwrap_err();
    assert!(
        matches!(error.downcast_ref(), Some(GalaxyError::BranchEmpty { .. })),
        "{error:#}"
    );
}

#[test]
fn an_install_resumed_after_the_follow_changed_is_refused() {
    let fx = Fixture::new();
    fx.install_stopped_without_pending(Choice::Branch("beta".to_owned()));
    let plan = fx.plan_with(&fx.request()).unwrap();
    let mut record = fx.record().unwrap();
    record.follow = Follow::default();
    fs::write(
        folder::sidecar_file(&fx.paths, &fx.folder),
        record.to_json().unwrap(),
    )
    .unwrap();
    let error = fx.apply(&plan).unwrap_err();
    assert!(
        matches!(
            error.downcast_ref(),
            Some(crate::gog::update::install::InstallError::Changed { .. })
        ),
        "{error:#}"
    );
    assert_eq!(fx.follows(), Follow::default());
}

fn dlc_depot() -> FakeDepot {
    FakeDepot::new(DLC).file(FakeFile::new("Data\\dlc.dat", b"bought"))
}

#[test]
fn a_pinned_game_installs_the_dlc_its_pinned_build_has_and_stays_pinned() {
    use crate::gog::update::{DlcState, check};
    let fx = Fixture::new();
    fx.gog.lock().unwrap().dlc = vec![DLC];
    fx.gog.lock().unwrap().owned = Some(vec![GAME]);
    fx.content.add_build(Fixture::game("100"));
    fx.install();
    // Build 100 is listed again with the DLC, behind a newer build 200.
    fx.gog.lock().unwrap().newest_first = true;
    fx.content
        .add_build(Fixture::game("100").depot(dlc_depot()));
    fx.content.add_build(game_200());
    fx.pin("100");
    assert!(matches!(
        fx.plan_choosing(Choice::Keep).unwrap(),
        Planned::Pinned(_)
    ));
    let status = check(&mut fx.session(), "game", true).unwrap();
    assert_eq!(status.state, UpdateState::Available);
    assert!(!status.applicable);
    assert!(matches!(status.reason, Some(Reason::Pinned { .. })));

    fx.gog.lock().unwrap().owned = Some(vec![GAME, DLC]);
    let status = check(&mut fx.session(), "game", true).unwrap();
    assert_eq!(status.state, UpdateState::Available);
    assert!(status.applicable);
    assert_eq!(status.reason, None);
    assert_eq!(status.pinned.as_deref(), Some("100"));
    let dlc = status.dlc.iter().find(|dlc| dlc.id == DLC).unwrap();
    assert_eq!(dlc.state, DlcState::NotInstalled);
    assert_eq!(dlc.available.as_deref(), Some("100"));

    let Planned::Update(plan) = fx.plan_choosing(Choice::Keep).unwrap() else {
        panic!("expected the DLC to install");
    };
    assert_eq!(plan.build(), "100");
    assert_eq!(plan.from(), plan.to());
    fx.apply_update(&plan).unwrap();
    assert_eq!(fx.read("Data/dlc.dat"), "bought");
    let committed = fx.state().committed.unwrap();
    assert_eq!(committed.products[0].build.as_deref(), Some("100"));
    assert_eq!(committed.products[1].status, ProductStatus::Installed);
    assert_eq!(fx.follows().pinned.as_deref(), Some("100"));

    // With nothing left to install, it's the pinned message again, and the
    // newer build is only held back.
    assert!(matches!(
        fx.plan_choosing(Choice::Keep).unwrap(),
        Planned::Pinned(_)
    ));
    let status = check(&mut fx.session(), "game", true).unwrap();
    assert!(!status.applicable);
    assert!(matches!(status.reason, Some(Reason::Pinned { .. })));
}

#[test]
fn a_pinned_game_is_not_offered_dlc_its_pinned_build_lacks() {
    use crate::gog::update::check;
    let fx = Fixture::new();
    fx.gog.lock().unwrap().dlc = vec![DLC];
    fx.gog.lock().unwrap().owned = Some(vec![GAME]);
    fx.content.add_build(Fixture::game("100"));
    fx.install();
    fx.gog.lock().unwrap().newest_first = true;
    // Only the newer build has the DLC.
    fx.content.add_build(game_200().depot(dlc_depot()));
    fx.pin("100");
    fx.gog.lock().unwrap().owned = Some(vec![GAME, DLC]);

    let status = check(&mut fx.session(), "game", true).unwrap();
    assert_eq!(status.state, UpdateState::Available);
    assert!(!status.applicable);
    assert!(matches!(status.reason, Some(Reason::Pinned { .. })));
    assert!(status.dlc.iter().all(|dlc| dlc.id != DLC));
    assert!(matches!(
        fx.plan_choosing(Choice::Keep).unwrap(),
        Planned::Pinned(_)
    ));
    assert_eq!(fx.state().committed.unwrap().products.len(), 1);
}
