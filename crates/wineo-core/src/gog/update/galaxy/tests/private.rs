//! Private branches: passwords asked for, saved once GOG lists the branch,
//! used by every later operation, and removed when refused.

use wineo_gog::Secret;

use super::update::game_200;
use super::*;
use crate::gog::update::apply::Options;
use crate::gog::update::galaxy::follow::Choice;
use crate::gog::update::galaxy::listing;
use crate::gog::update::galaxy::private;
use crate::gog::update::galaxy::record::Follow;
use crate::gog::update::galaxy::update::{self, Planned};
use crate::gog::update::{Reason, UpdateState};

const PASSWORD: &str = "hunter2-branch";

fn secret(text: &str) -> Secret {
    Secret::new(text)
}

impl Fixture {
    /// Build 100 installed, with a private beta branch at 300 that needs
    /// `PASSWORD` to be listed.
    fn with_private_beta() -> Self {
        let fx = Self::new();
        fx.content.add_build(Fixture::game("100"));
        fx.install();
        fx.content.add_build(Fixture::game("300").branch("beta"));
        fx.content.set_branch_password("beta", PASSWORD);
        fx
    }

    fn saved_password(&self, branch: &str) -> Option<String> {
        self.keyring
            .1
            .lock()
            .unwrap()
            .get(&(USER.to_string(), GAME, branch.to_owned()))
            .cloned()
    }

    fn save_password(&self, branch: &str, password: &str) {
        self.keyring.1.lock().unwrap().insert(
            (USER.to_string(), GAME, branch.to_owned()),
            password.to_owned(),
        );
    }

    fn plan_with_options(&self, options: Options) -> Result<Planned> {
        update::plan(
            &self.paths,
            &mut self.session(),
            "game",
            Options {
                refresh: true,
                ..options
            },
            &Token::new(),
        )
    }

    fn plan_branch(&self, branch: &str, password: Option<&str>) -> Result<Planned> {
        self.plan_with_options(Options {
            choice: Choice::Branch(branch.to_owned()),
            password: password.map(secret),
            ..Options::default()
        })
    }

    /// Follows the private beta branch, with its password saved.
    fn following_private_beta() -> Self {
        let fx = Self::with_private_beta();
        let Planned::Update(plan) = fx.plan_branch("beta", Some(PASSWORD)).unwrap() else {
            panic!("expected an update");
        };
        fx.apply_update(&plan).unwrap();
        assert_eq!(fx.saved_password("beta").as_deref(), Some(PASSWORD));
        fx
    }
}

fn beta() -> Follow {
    Follow {
        branch: Some("beta".to_owned()),
        pinned: None,
    }
}

fn galaxy_error(error: &anyhow::Error) -> &GalaxyError {
    error
        .downcast_ref::<GalaxyError>()
        .unwrap_or_else(|| panic!("{error:#}"))
}

/// No error, warning, or debug text may hold the password.
fn assert_quiet(text: &str) {
    assert!(!text.contains(PASSWORD), "{text}");
    assert!(!text.contains("password="), "{text}");
}

#[test]
fn a_private_branch_asks_for_its_password_and_changes_nothing_while_it_waits() {
    let fx = Fixture::with_private_beta();
    let error = fx.plan_branch("beta", None).unwrap_err();
    let GalaxyError::NeedsPassword {
        title,
        branch,
        command,
    } = galaxy_error(&error)
    else {
        panic!("{error:#}");
    };
    assert_eq!((title.as_str(), branch.as_str()), ("Fake Game", "beta"));
    assert_eq!(command, "wineo update game --branch beta");
    assert_eq!(
        error.to_string(),
        "Fake Game's branch beta is private. Enter its password with: wineo update game --branch beta"
    );
    assert_eq!(fx.saved_password("beta"), None);
    assert_eq!(fx.record().unwrap().follow, Follow::default());
}

#[test]
fn a_command_to_enter_a_password_quotes_a_branch_name_with_spaces() {
    let fx = Fixture::with_private_beta();
    fx.content
        .add_build(Fixture::game("400").branch("Legacy (64-bit)"));
    fx.content
        .set_branch_password("Legacy (64-bit)", "other-pass");
    let error = fx.plan_branch("Legacy (64-bit)", None).unwrap_err();
    assert!(
        error
            .to_string()
            .ends_with("wineo update game --branch 'Legacy (64-bit)'"),
        "{error:#}"
    );
}

#[test]
fn an_accepted_password_is_saved_and_the_update_follows_the_branch() {
    let fx = Fixture::with_private_beta();
    let Planned::Update(plan) = fx.plan_branch("beta", Some(PASSWORD)).unwrap() else {
        panic!("expected an update");
    };
    assert_eq!(plan.build(), "300");
    assert_eq!(plan.to(), &beta());
    assert_eq!(fx.saved_password("beta").as_deref(), Some(PASSWORD));
    assert!(!format!("{plan:?}").contains(PASSWORD));
    fx.apply_update(&plan).unwrap();
    assert_eq!(fx.record().unwrap().follow, beta());
}

#[test]
fn a_saved_password_is_used_without_asking_and_entering_one_again_replaces_it() {
    let fx = Fixture::following_private_beta();
    // Back to the default branch, then to beta with nothing entered.
    let Planned::Update(back) = fx
        .plan_with_options(Options {
            choice: Choice::DefaultBranch,
            ..Options::default()
        })
        .unwrap()
    else {
        panic!("expected an update");
    };
    fx.apply_update(&back).unwrap();
    assert!(matches!(
        fx.plan_branch("beta", None).unwrap(),
        Planned::Update(_)
    ));

    // The owner changed the password: the saved one is refused and goes,
    // and the new one entered takes its place.
    fx.content.set_branch_password("beta", "a-new-one");
    let error = fx.plan_branch("beta", None).unwrap_err();
    assert!(matches!(galaxy_error(&error), GalaxyError::Refused { .. }));
    assert_eq!(fx.saved_password("beta"), None);
    fx.save_password("beta", PASSWORD);
    assert!(matches!(
        fx.plan_branch("beta", Some("a-new-one")).unwrap(),
        Planned::Update(_)
    ));
    assert_eq!(fx.saved_password("beta").as_deref(), Some("a-new-one"));
}

#[test]
fn a_refused_password_is_not_saved_and_leaks_nothing() {
    let fx = Fixture::with_private_beta();
    let error = fx.plan_branch("beta", Some("wrong-one")).unwrap_err();
    assert_eq!(
        error.to_string(),
        "GOG lists no branch beta for Fake Game, or refused its password."
    );
    assert_eq!(fx.saved_password("beta"), None);
    for secret in ["wrong-one", PASSWORD] {
        let shown = format!("{error:#} {error:?}");
        assert!(!shown.contains(secret), "{shown}");
    }
}

#[test]
fn only_the_password_that_was_refused_is_removed() {
    let fx = Fixture::with_private_beta();
    // A wrong one entered doesn't say the one saved is wrong.
    fx.save_password("beta", PASSWORD);
    let error = fx.plan_branch("beta", Some("wrong-one")).unwrap_err();
    assert!(matches!(galaxy_error(&error), GalaxyError::Refused { .. }));
    assert_eq!(fx.saved_password("beta").as_deref(), Some(PASSWORD));

    // The same one entered and refused is the saved one refused.
    fx.content.set_branch_password("beta", "changed-by-owner");
    let error = fx.plan_branch("beta", Some(PASSWORD)).unwrap_err();
    assert!(matches!(galaxy_error(&error), GalaxyError::Refused { .. }));
    assert_eq!(fx.saved_password("beta"), None);
}

/// The list GOG gave for the saved password, which the owner has since
/// changed, so the branch is missing from it.
fn refused_fetch(fx: &Fixture, session: &mut Session) -> private::Fetched {
    fx.save_password("beta", PASSWORD);
    fx.content.set_branch_password("beta", "changed-by-owner");
    let fetched = private::fetch(session, GAME, Some("beta"), None).unwrap();
    assert_eq!(fetched.sent, private::Sent::Saved);
    assert!(!fetched.lists("beta"));
    fetched
}

fn game<'a>() -> private::Game<'a> {
    private::Game {
        title: "Fake Game",
        gog_id: GAME,
        name: Some("game"),
    }
}

#[test]
fn a_refusal_that_a_newer_saved_password_overtook_removes_nothing() {
    let fx = Fixture::with_private_beta();
    let mut session = fx.session();
    let fetched = refused_fetch(&fx, &mut session);
    // Another process saves the branch's new password after the request.
    fx.save_password("beta", "the-new-password");
    let error = fetched
        .require(&mut session, &game(), "beta", false)
        .unwrap_err();
    assert!(matches!(galaxy_error(&error), GalaxyError::Refused { .. }));
    assert_eq!(
        fx.saved_password("beta").as_deref(),
        Some("the-new-password")
    );
    // The check and the listing remove through the same call.
    assert!(!fetched.forget_refused(&mut session, GAME, "beta").unwrap());
    assert_eq!(
        fx.saved_password("beta").as_deref(),
        Some("the-new-password")
    );
}

#[test]
fn a_refusal_that_another_account_s_login_overtook_removes_nothing() {
    let fx = Fixture::with_private_beta();
    let mut session = fx.session();
    let fetched = refused_fetch(&fx, &mut session);
    // Another account logs in, which clears the passwords, and saves its own.
    crate::gog::account::store(
        &fx.paths,
        &*fx.keyring,
        &account::StoredLogin {
            refresh_token: "OTHER".to_owned(),
            user_id: wineo_gog::UserId::new(USER + 1),
            username: None,
        },
    )
    .unwrap();
    fx.save_password("beta", "their-password");
    let error = fetched
        .require(&mut session, &game(), "beta", false)
        .unwrap_err();
    assert!(
        matches!(
            error.downcast_ref::<account::AccountError>(),
            Some(account::AccountError::Switched)
        ),
        "{error:#}"
    );
    assert_eq!(fx.saved_password("beta").as_deref(), Some("their-password"));
    assert!(matches!(
        fetched.forget_refused(&mut session, GAME, "beta"),
        Err(account::AccountError::Switched)
    ));
    assert_eq!(fx.saved_password("beta").as_deref(), Some("their-password"));
}

#[test]
fn a_typo_on_a_game_without_private_branches_is_no_branch_and_asks_for_nothing() {
    let fx = Fixture::on_branches_public();
    let error = fx.plan_branch("bata", None).unwrap_err();
    assert!(
        matches!(galaxy_error(&error), GalaxyError::NoBranch { .. }),
        "{error:#}"
    );
    assert_eq!(
        error.to_string(),
        "GOG lists no branch bata for Fake Game. See: wineo update game --builds"
    );
}

impl Fixture {
    fn on_branches_public() -> Self {
        let fx = Self::new();
        fx.content.add_build(Fixture::game("100"));
        fx.install();
        fx.content.add_build(Fixture::game("300").branch("beta"));
        fx
    }
}

#[test]
fn a_typo_on_a_game_with_private_branches_asks_and_then_says_it_may_be_refused() {
    let fx = Fixture::with_private_beta();
    let error = fx.plan_branch("bata", None).unwrap_err();
    assert!(matches!(
        galaxy_error(&error),
        GalaxyError::NeedsPassword { .. }
    ));
    let error = fx.plan_branch("bata", Some(PASSWORD)).unwrap_err();
    assert_eq!(
        error.to_string(),
        "GOG lists no branch bata for Fake Game, or refused its password."
    );
    assert_eq!(fx.saved_password("bata"), None);
    // The real branch's password is not touched by a typo.
    assert_eq!(fx.saved_password("beta"), None);
}

#[test]
fn a_dry_run_saves_and_removes_nothing() {
    let fx = Fixture::with_private_beta();
    let dry = |password: Option<&str>| Options {
        choice: Choice::Branch("beta".to_owned()),
        password: password.map(secret),
        read_only: true,
        ..Options::default()
    };
    assert!(matches!(
        fx.plan_with_options(dry(Some(PASSWORD))).unwrap(),
        Planned::Update(_)
    ));
    assert_eq!(fx.saved_password("beta"), None);
    fx.save_password("beta", "stale-password");
    fx.plan_with_options(dry(Some("wrong"))).unwrap_err();
    assert_eq!(fx.saved_password("beta").as_deref(), Some("stale-password"));
}

#[test]
fn updates_reinstalls_and_repairs_of_a_followed_private_branch_use_the_saved_password() {
    let fx = Fixture::following_private_beta();
    fx.content.add_build(Fixture::game("400").branch("beta"));
    fx.gog.lock().unwrap().newest_first = true;

    // A plain update goes to the branch's newest build.
    let Planned::Update(plan) = fx.plan_with_options(Options::default()).unwrap() else {
        panic!("expected an update");
    };
    assert_eq!(plan.build(), "400");
    assert_eq!(plan.to(), &beta());

    let Planned::Update(plan) = fx
        .plan_with_options(Options {
            reinstall: true,
            ..Options::default()
        })
        .unwrap()
    else {
        panic!("expected a reinstall");
    };
    assert_eq!(plan.build(), "400");

    let Planned::Update(plan) = fx
        .plan_with_options(Options {
            choice: Choice::Latest,
            ..Options::default()
        })
        .unwrap()
    else {
        panic!("expected an update");
    };
    assert_eq!(plan.build(), "400");

    // The password was sent every time, and never anywhere but a query.
    let sent = fx
        .server
        .requests()
        .iter()
        .filter(|request| request.path().ends_with("/os/windows/builds"))
        .filter(|request| request.query("password").as_deref() == Some(PASSWORD))
        .count();
    assert!(sent >= 3, "{sent}");
}

#[test]
fn a_repair_without_a_stored_manifest_fetches_the_branch_with_the_saved_password() {
    let fx = Fixture::following_private_beta();
    // Forget the stored manifest, so the repair has to ask GOG for the build.
    let mut record = fx.record().unwrap();
    record.installed = None;
    fs::write(
        folder::sidecar_file(&fx.paths, &fx.folder),
        record.to_json().unwrap(),
    )
    .unwrap();
    let repair = Options {
        repair: true,
        ..Options::default()
    };
    let Planned::Update(plan) = fx.plan_with_options(repair.clone()).unwrap() else {
        panic!("expected a repair");
    };
    assert_eq!(plan.build(), "300");

    // Without the password, it asks for it.
    fx.keyring.1.lock().unwrap().clear();
    let error = fx.plan_with_options(repair).unwrap_err();
    assert!(matches!(
        galaxy_error(&error),
        GalaxyError::NeedsPassword { .. }
    ));
}

#[test]
fn a_followed_private_branch_without_its_password_asks_for_it_again() {
    let fx = Fixture::following_private_beta();
    fx.keyring.1.lock().unwrap().clear();
    let error = fx.plan_with_options(Options::default()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Fake Game's branch beta is private. Enter its password with: wineo update game --branch beta"
    );
    // Entering it plans the update, and saves it once accepted.
    fx.content.add_build(Fixture::game("400").branch("beta"));
    fx.gog.lock().unwrap().newest_first = true;
    let Planned::Update(plan) = fx
        .plan_with_options(Options {
            password: Some(secret(PASSWORD)),
            ..Options::default()
        })
        .unwrap()
    else {
        panic!("expected an update");
    };
    assert_eq!(plan.build(), "400");
    assert_eq!(fx.saved_password("beta").as_deref(), Some(PASSWORD));
}

#[test]
fn a_revoked_password_on_a_followed_branch_is_removed_when_an_update_finds_it_refused() {
    let fx = Fixture::following_private_beta();
    fx.content.set_branch_password("beta", "changed-by-owner");
    let error = fx.plan_with_options(Options::default()).unwrap_err();
    assert!(matches!(galaxy_error(&error), GalaxyError::Refused { .. }));
    assert_eq!(fx.saved_password("beta"), None);
}

#[test]
fn a_game_following_a_public_branch_never_asks_for_a_password() {
    let fx = Fixture::on_branches_public();
    let Planned::Update(plan) = fx.plan_branch("beta", None).unwrap() else {
        panic!("expected an update");
    };
    fx.apply_update(&plan).unwrap();
    // Even with private branches about, a listed branch needs none.
    fx.content.set_private_branches(true);
    assert!(matches!(
        fx.plan_with_options(Options::default()).unwrap(),
        Planned::UpToDate { .. }
    ));
}

#[test]
fn a_check_of_a_followed_private_branch_uses_the_saved_password_or_cannot_compare() {
    let fx = Fixture::following_private_beta();
    fx.content.add_build(Fixture::game("400").branch("beta"));
    fx.gog.lock().unwrap().newest_first = true;
    let check = |fx: &Fixture| crate::gog::update::check(&mut fx.session(), "game", true).unwrap();
    let status = check(&fx);
    assert_eq!(status.state, UpdateState::Available);
    assert_eq!(status.available.as_deref(), Some("400"));
    assert_eq!(status.branch.as_deref(), Some("beta"));

    fx.keyring.1.lock().unwrap().clear();
    let status = check(&fx);
    assert_eq!(status.state, UpdateState::CantCompare);
    assert!(!status.applicable);
    assert_eq!(status.branch.as_deref(), Some("beta"));
    let reason = status.reason.expect("a reason");
    assert_eq!(
        reason,
        Reason::Password(
            "Enter its password again with: wineo update game --branch beta".to_owned()
        )
    );
    // The JSON value is the sentence, as for any other can't-compare reason.
    assert_eq!(
        reason.as_str(),
        "Enter its password again with: wineo update game --branch beta"
    );
}

#[test]
fn the_listing_adds_the_builds_of_branches_with_saved_passwords_marked_private() {
    let fx = Fixture::following_private_beta();
    fx.content.add_build(game_200());
    fx.gog.lock().unwrap().newest_first = true;
    let list = |fx: &Fixture, entered: Option<(&str, &Secret)>| {
        listing::list(&fx.paths, &mut fx.session(), "game", entered, true, false).unwrap()
    };
    let seen = |listing: &listing::Listing| -> Vec<(Option<String>, String, Vec<&'static str>)> {
        listing
            .builds
            .iter()
            .map(|build| (build.branch.clone(), build.build.clone(), build.marks()))
            .collect()
    };

    let listed = list(&fx, None);
    assert!(listed.private_branches);
    assert_eq!(
        seen(&listed),
        [
            (None, "200".to_owned(), vec!["latest"]),
            (None, "100".to_owned(), vec![]),
            (
                Some("beta".to_owned()),
                "300".to_owned(),
                vec!["installed", "latest", "followed", "private"]
            ),
        ]
    );
    assert!(listed.builds[2].private && !listed.builds[0].private);

    // Without the saved password, the branch isn't listed.
    fx.keyring.1.lock().unwrap().clear();
    let listed = list(&fx, None);
    assert!(listed.private_branches);
    assert_eq!(listed.builds.len(), 2);
    assert!(listed.builds.iter().all(|build| !build.private));

    // A password given for the call lists the branch and is saved.
    let given = secret(PASSWORD);
    let listed = list(&fx, Some(("beta", &given)));
    assert_eq!(listed.builds.len(), 3);
    assert_eq!(fx.saved_password("beta").as_deref(), Some(PASSWORD));
}

#[test]
fn the_listing_removes_a_saved_password_its_branch_does_not_answer_to() {
    let fx = Fixture::following_private_beta();
    fx.content.set_branch_password("beta", "changed-by-owner");
    let listed = listing::list(&fx.paths, &mut fx.session(), "game", None, true, false).unwrap();
    assert_eq!(listed.builds.len(), 1);
    assert_eq!(
        listed.warnings,
        ["GOG lists no branch beta for Fake Game, or refused its password."]
    );
    assert_eq!(fx.saved_password("beta"), None);

    // A read-only listing says so but keeps what it was given.
    fx.save_password("beta", PASSWORD);
    let listed = listing::list(&fx.paths, &mut fx.session(), "game", None, true, true).unwrap();
    assert_eq!(listed.warnings.len(), 1);
    assert_eq!(fx.saved_password("beta").as_deref(), Some(PASSWORD));
}

#[test]
fn an_install_on_a_private_branch_asks_for_the_password_and_saves_it() {
    let fx = Fixture::new();
    fx.content.add_build(Fixture::game("100"));
    fx.content.add_build(Fixture::game("300").branch("beta"));
    fx.content.set_branch_password("beta", PASSWORD);
    let mut request = fx.request();
    request.builds = Choice::Branch("beta".to_owned());
    let error = fx.plan_with(&request).unwrap_err();
    let GalaxyError::NeedsPassword { command, .. } = galaxy_error(&error) else {
        panic!("{error:#}");
    };
    assert_eq!(
        command,
        &format!("wineo import gog --game {GAME} --branch beta")
    );

    request.password = Some(secret("wrong"));
    let error = fx.plan_with(&request).unwrap_err();
    assert!(matches!(galaxy_error(&error), GalaxyError::Refused { .. }));
    assert_eq!(fx.saved_password("beta"), None);

    request.password = Some(secret(PASSWORD));
    let plan = fx.plan_with(&request).unwrap();
    assert_eq!(fx.saved_password("beta").as_deref(), Some(PASSWORD));
    let installed = fx.apply(&plan).unwrap();
    assert_eq!(installed.environment, "game");
    assert_eq!(fx.record().unwrap().follow, beta());
}

#[test]
fn the_password_is_never_in_an_error_or_a_plan() {
    let fx = Fixture::with_private_beta();
    let mut shown = Vec::new();
    for password in [Some(PASSWORD), Some("wrong-secret"), None] {
        match fx.plan_branch("beta", password) {
            Ok(plan) => shown.push(format!("{plan:?}")),
            Err(error) => shown.push(format!("{error:#} {error:?}")),
        }
        match fx.plan_branch("nope", password) {
            Ok(plan) => shown.push(format!("{plan:?}")),
            Err(error) => shown.push(format!("{error:#} {error:?}")),
        }
    }
    let options = Options {
        password: Some(secret(PASSWORD)),
        ..Options::default()
    };
    shown.push(format!("{options:?}"));
    shown.push(format!(
        "{:?}",
        Request {
            password: Some(secret(PASSWORD)),
            ..fx.request()
        }
    ));
    for text in shown {
        assert_quiet(&text);
        assert!(!text.contains("wrong-secret"), "{text}");
    }
}

#[test]
fn a_check_finding_the_saved_password_refused_says_so_and_removes_it() {
    let fx = Fixture::following_private_beta();
    fx.content.set_branch_password("beta", "changed-by-owner");
    let check = |fx: &Fixture| crate::gog::update::check(&mut fx.session(), "game", true).unwrap();

    // A dry run leaves the password where it was.
    let status = crate::gog::update::check_with(
        &mut fx.session(),
        "game",
        crate::gog::update::Check {
            refresh: true,
            read_only: true,
        },
    )
    .unwrap();
    assert_eq!(status.state, UpdateState::CantCompare);
    assert_eq!(fx.saved_password("beta").as_deref(), Some(PASSWORD));

    let status = check(&fx);
    assert_eq!(status.state, UpdateState::CantCompare);
    assert!(!status.applicable);
    assert_eq!(status.branch.as_deref(), Some("beta"));
    assert_eq!(
        status.reason,
        Some(Reason::Password(
            "GOG lists no branch beta for Fake Game, or refused its password.".to_owned()
        ))
    );
    assert_eq!(fx.saved_password("beta"), None);

    // With the password gone, the next check asks for it again.
    let status = check(&fx);
    assert_eq!(
        status.reason,
        Some(Reason::Password(
            "Enter its password again with: wineo update game --branch beta".to_owned()
        ))
    );
}

impl Fixture {
    /// Every builds request that carried a password.
    fn password_requests(&self) -> usize {
        self.server
            .requests()
            .iter()
            .filter(|request| request.path().ends_with("/os/windows/builds"))
            .filter(|request| request.query("password").is_some())
            .count()
    }

    /// Another account logs in while the keyring can't be reached to wipe.
    fn switch_account_with_a_failed_wipe(&self) {
        self.keyring
            .2
            .store(true, std::sync::atomic::Ordering::SeqCst);
        crate::gog::account::store(
            &self.paths,
            &*self.keyring,
            &account::StoredLogin {
                refresh_token: "OTHER".to_owned(),
                user_id: wineo_gog::UserId::new(USER + 1),
                username: None,
            },
        )
        .unwrap();
        // The wipe changed nothing, so the first account's item is still there.
        assert_eq!(self.saved_password("beta").as_deref(), Some(PASSWORD));
    }
}

#[test]
fn after_a_switch_whose_wipe_failed_the_new_account_never_sends_or_lists_the_old_one_s_password() {
    let fx = Fixture::following_private_beta();
    fx.content.add_build(Fixture::game("400").branch("beta"));
    fx.gog.lock().unwrap().newest_first = true;
    let before = fx.password_requests();
    fx.switch_account_with_a_failed_wipe();

    // A check can't compare, as with no password, and sends none.
    let status = crate::gog::update::check(&mut fx.session(), "game", true).unwrap();
    assert_eq!(status.state, UpdateState::CantCompare);
    assert_eq!(
        status.reason,
        Some(Reason::Password(
            "Enter its password again with: wineo update game --branch beta".to_owned()
        ))
    );
    // An update asks for the password.
    let error = fx.plan_with_options(Options::default()).unwrap_err();
    assert!(matches!(
        galaxy_error(&error),
        GalaxyError::NeedsPassword { .. }
    ));
    // A listing has only the public builds.
    let listed = listing::list(&fx.paths, &mut fx.session(), "game", None, true, false).unwrap();
    assert!(listed.builds.iter().all(|build| build.branch.is_none()));
    assert!(listed.builds.iter().all(|build| !build.private));
    assert_eq!(fx.password_requests(), before, "a password was sent");

    // The new account entering its own clears the old one out.
    let Planned::Update(_) = fx.plan_branch("beta", Some(PASSWORD)).unwrap() else {
        panic!("expected an update");
    };
    let owners: Vec<String> = fx
        .keyring
        .1
        .lock()
        .unwrap()
        .keys()
        .map(|(owner, _, _)| owner.clone())
        .collect();
    assert_eq!(owners, [(USER + 1).to_string()]);
}

#[test]
fn an_item_with_no_owner_is_never_sent_and_goes_with_the_next_write() {
    let fx = Fixture::with_private_beta();
    fx.keyring.1.lock().unwrap().insert(
        (String::new(), GAME, "beta".to_owned()),
        PASSWORD.to_owned(),
    );
    let error = fx.plan_branch("beta", None).unwrap_err();
    assert!(matches!(
        galaxy_error(&error),
        GalaxyError::NeedsPassword { .. }
    ));
    assert_eq!(fx.password_requests(), 0);
    fx.plan_branch("beta", Some(PASSWORD)).unwrap();
    assert!(
        fx.keyring
            .1
            .lock()
            .unwrap()
            .keys()
            .all(|(owner, _, _)| owner == &USER.to_string())
    );
}
