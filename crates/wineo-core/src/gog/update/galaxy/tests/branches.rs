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
