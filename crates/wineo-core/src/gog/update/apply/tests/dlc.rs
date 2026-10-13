//! DLC an update installs that the folder doesn't have installed: newly
//! owned DLC, and failed or unavailable DLC GOG offers now.

use super::*;
use crate::gog::update::{DlcState, DlcStatus, Status, check};

impl Fixture {
    fn check(&self) -> Status {
        check(&mut self.session(), "rimworld", true).unwrap()
    }

    fn owns_dlc(&self, ids: &[u64]) {
        self.gog.lock().unwrap().owned = Some(ids.to_vec());
    }

    /// RimWorld 1 installed with nothing else, and GOG offering it.
    fn base_only() -> Self {
        let fx = Self::new();
        fx.offer(RIMWORLD, "RimWorld", "1", &[file("start.sh", "s")]);
        fx.owns_dlc(&[RIMWORLD]);
        fx.migrated(&[(RIMWORLD, Some("1"))]);
        fx.reinstall();
        fx
    }

    /// Records `id` after the base with `status`, as an install that couldn't
    /// install it leaves it.
    fn recorded(&self, id: u64, status: ProductStatus) {
        let mut state = self.state();
        state.committed.as_mut().unwrap().products.push(Product {
            reported_language: None,
            id,
            version: None,
            build: None,
            language: None,
            status,
        });
        self.save(&state);
    }

    fn products(&self) -> Vec<(u64, ProductStatus, Option<String>)> {
        self.state()
            .committed
            .unwrap()
            .products
            .iter()
            .map(|product| (product.id, product.status, product.version.clone()))
            .collect()
    }
}

fn dlc_states(status: &Status) -> Vec<(u64, DlcState, Option<&str>)> {
    status
        .dlc
        .iter()
        .map(|dlc: &DlcStatus| (dlc.id, dlc.state, dlc.available.as_deref()))
        .collect()
}

fn one(version: &str) -> Option<String> {
    Some(version.to_owned())
}

#[test]
fn a_new_dlc_state_reads_not_installed_in_json() {
    assert_eq!(DlcState::NotInstalled.as_str(), "not_installed");
    assert_eq!(DlcState::Failed.as_str(), "failed");
    assert_eq!(DlcState::Unavailable.as_str(), "unavailable");
}

#[test]
fn newly_owned_dlc_is_found_by_a_check_and_installed_by_update_alone() {
    let fx = Fixture::base_only();
    fx.offer(ROYALTY, "Royalty", "1", &[file("r", "royalty")]);
    // GOG lists it, but it isn't owned yet.
    let status = fx.check();
    assert_eq!(status.state, UpdateState::UpToDate);
    assert!(status.dlc.is_empty());
    assert!(matches!(
        fx.plan_with(true, Options::default()).unwrap(),
        Plan::UpToDate { .. }
    ));

    fx.owns_dlc(&[RIMWORLD, ROYALTY]);
    let status = fx.check();
    // The game is current; the DLC alone makes the update.
    assert_eq!(status.state, UpdateState::Available);
    assert!(status.applicable);
    assert_eq!(
        (status.installed.as_deref(), status.available.as_deref()),
        (Some("1"), Some("1"))
    );
    assert_eq!(
        dlc_states(&status),
        [(ROYALTY, DlcState::NotInstalled, Some("1"))]
    );
    assert_eq!(status.dlc[0].title.as_deref(), Some("Royalty"));
    assert!(status.warnings.is_empty(), "{:?}", status.warnings);

    let pending = fx.pending(true, Options::default());
    let planned: Vec<_> = pending
        .products()
        .iter()
        .map(|product| {
            (
                product.id,
                product.installed.clone(),
                product.version.clone(),
            )
        })
        .collect();
    assert_eq!(
        planned,
        [(RIMWORLD, one("1"), one("1")), (ROYALTY, None, one("1"))]
    );
    let updated = fx
        .apply_with(true, &pending, &Token::new(), |_| {})
        .unwrap();
    assert!(updated.warnings.is_empty(), "{:?}", updated.warnings);
    assert_eq!(fx.read("r"), "royalty");
    assert_eq!(
        fx.products(),
        [
            (RIMWORLD, ProductStatus::Installed, one("1")),
            (ROYALTY, ProductStatus::Installed, one("1")),
        ]
    );
    let status = fx.check();
    assert_eq!(status.state, UpdateState::UpToDate);
    assert_eq!(
        dlc_states(&status),
        [(ROYALTY, DlcState::UpToDate, Some("1"))]
    );
}

#[test]
fn a_reinstall_installs_newly_owned_dlc_too() {
    let fx = Fixture::base_only();
    fx.offer(ROYALTY, "Royalty", "1", &[file("r", "royalty")]);
    fx.owns_dlc(&[RIMWORLD, ROYALTY]);
    fx.reinstall();
    assert_eq!(fx.read("r"), "royalty");
    assert_eq!(fx.products()[1].1, ProductStatus::Installed);
}

#[test]
fn without_the_owned_ids_nothing_new_is_claimed() {
    let fx = Fixture::base_only();
    fx.offer(ROYALTY, "Royalty", "1", &[file("r", "royalty")]);
    fx.gog.lock().unwrap().owned_ids_fail = true;
    let status = fx.check();
    assert_eq!(status.state, UpdateState::UpToDate);
    assert!(status.dlc.is_empty());
    assert_eq!(status.warnings.len(), 1);
    assert!(
        status.warnings[0].starts_with("Couldn't check which DLC you own"),
        "{:?}",
        status.warnings
    );
    assert!(matches!(
        fx.plan_with(true, Options::default()).unwrap(),
        Plan::UpToDate { .. }
    ));

    // An update for the game itself still runs, without the DLC, and says
    // why.
    fx.offer(RIMWORLD, "RimWorld", "2", &[file("start.sh", "s2")]);
    let pending = fx.pending(true, Options::default());
    assert_eq!(pending.products().len(), 1);
    assert!(
        pending
            .warnings()
            .iter()
            .any(|warning| warning.starts_with("Couldn't check which DLC you own")),
        "{:?}",
        pending.warnings()
    );
    fx.apply_with(true, &pending, &Token::new(), |_| {})
        .unwrap();
    assert_eq!(
        fx.products(),
        [(RIMWORLD, ProductStatus::Installed, one("2"))]
    );
    assert!(!fx.folder.join("r").exists());
}

#[test]
fn a_game_whose_dlc_are_all_recorded_never_asks_which_it_owns() {
    let fx = Fixture::base_only();
    fx.offer(ROYALTY, "Royalty", "1", &[file("r", "royalty")]);
    fx.recorded(ROYALTY, ProductStatus::Unavailable);
    fx.withdraw(ROYALTY);
    fx.gog.lock().unwrap().owned_ids_fail = true;
    let status = fx.check();
    assert!(status.warnings.is_empty(), "{:?}", status.warnings);
    assert_eq!(
        dlc_states(&status),
        [(ROYALTY, DlcState::Unavailable, None)]
    );
}

#[test]
fn a_failed_dlc_is_retried_and_installs() {
    let fx = Fixture::base_only();
    fx.offer(ROYALTY, "Royalty", "1", &[file("r", "royalty")]);
    fx.recorded(ROYALTY, ProductStatus::Failed);
    let status = fx.check();
    assert_eq!(status.state, UpdateState::Available);
    assert_eq!(
        dlc_states(&status),
        [(ROYALTY, DlcState::Failed, Some("1"))]
    );

    let updated = fx.update();
    assert!(updated.warnings.is_empty(), "{:?}", updated.warnings);
    assert_eq!(fx.read("r"), "royalty");
    assert_eq!(
        fx.products()[1],
        (ROYALTY, ProductStatus::Installed, one("1"))
    );
    assert_eq!(fx.check().state, UpdateState::UpToDate);
}

#[test]
fn a_failed_dlc_failing_again_stays_failed_and_the_game_launchable() {
    let fx = Fixture::base_only();
    fx.offer(ROYALTY, "Royalty", "1", &[file("r", "royalty")]);
    fx.recorded(ROYALTY, ProductStatus::Failed);
    fx.gog.lock().unwrap().failing = vec![ROYALTY];
    let updated = fx.update();
    assert_eq!(updated.warnings.len(), 1, "{:?}", updated.warnings);
    assert!(
        updated.warnings[0].starts_with(&format!(
            "Royalty ({ROYALTY}) failed and wasn't installed: "
        )),
        "{:?}",
        updated.warnings
    );
    assert_eq!(fx.state().journal, None);
    assert_eq!(fx.products()[1], (ROYALTY, ProductStatus::Failed, None));
    assert!(!fx.folder.join("r").exists());
    assert_eq!(fx.read("start.sh"), "s");
    // Still offered, so the next check still offers the retry.
    assert_eq!(fx.check().state, UpdateState::Available);

    // A newly owned one fails the same way, and is recorded as failed.
    let fx = Fixture::base_only();
    fx.offer(ROYALTY, "Royalty", "1", &[file("r", "royalty")]);
    fx.owns_dlc(&[RIMWORLD, ROYALTY]);
    fx.gog.lock().unwrap().failing = vec![ROYALTY];
    fx.update();
    assert_eq!(fx.products()[1], (ROYALTY, ProductStatus::Failed, None));
}

#[test]
fn a_package_an_installed_product_cant_get_still_refuses_the_update() {
    let fx = Fixture::base_only();
    fx.offer(ROYALTY, "Royalty", "1", &[file("r", "royalty")]);
    fx.owns_dlc(&[RIMWORLD, ROYALTY]);
    fx.update();
    fx.offer(ROYALTY, "Royalty", "2", &[file("r", "royalty 2")]);
    fx.gog.lock().unwrap().failing = vec![ROYALTY];
    let before = (fx.state(), fx.tree());
    fx.try_update(Options::default()).unwrap_err();
    assert_eq!((fx.state(), fx.tree()), before);
}

#[test]
fn an_unavailable_dlc_installs_once_gog_offers_it() {
    let fx = Fixture::base_only();
    fx.offer(ROYALTY, "Royalty", "1", &[file("r", "royalty")]);
    fx.withdraw(ROYALTY);
    fx.recorded(ROYALTY, ProductStatus::Unavailable);
    let status = fx.check();
    assert_eq!(status.state, UpdateState::UpToDate);
    assert_eq!(
        dlc_states(&status),
        [(ROYALTY, DlcState::Unavailable, None)]
    );
    assert!(matches!(
        fx.plan_with(true, Options::default()).unwrap(),
        Plan::UpToDate { .. }
    ));

    fx.offer(ROYALTY, "Royalty", "1", &[file("r", "royalty")]);
    let status = fx.check();
    assert_eq!(status.state, UpdateState::Available);
    assert_eq!(
        dlc_states(&status),
        [(ROYALTY, DlcState::NotInstalled, Some("1"))]
    );
    fx.update();
    assert_eq!(fx.read("r"), "royalty");
    assert_eq!(
        fx.products()[1],
        (ROYALTY, ProductStatus::Installed, one("1"))
    );
}

#[test]
fn a_dlc_offered_only_in_english_installs_in_english() {
    let fx = Fixture::base_only();
    let mut state = fx.state();
    state.committed.as_mut().unwrap().language = "fr".to_owned();
    fx.save(&state);
    {
        let mut gog = fx.gog.lock().unwrap();
        gog.offers.get_mut(&RIMWORLD).unwrap().language = "fr";
    }
    fx.offer(ROYALTY, "Royalty", "1", &[file("r", "royalty")]);
    fx.owns_dlc(&[RIMWORLD, ROYALTY]);
    assert_eq!(fx.check().state, UpdateState::Available);
    fx.update();
    let committed = fx.state().committed.unwrap();
    assert_eq!(committed.products[1].language.as_deref(), Some("en"));
    assert_eq!(fx.check().state, UpdateState::UpToDate);
}

#[test]
fn newly_owned_dlc_with_no_linux_installer_is_listed_and_recorded_unavailable() {
    let fx = Fixture::base_only();
    fx.offer(ROYALTY, "Royalty", "1", &[file("r", "royalty")]);
    fx.withdraw(ROYALTY);
    fx.owns_dlc(&[RIMWORLD, ROYALTY]);
    let status = fx.check();
    // Nothing to install, so no update for it alone.
    assert_eq!(status.state, UpdateState::UpToDate);
    assert_eq!(
        dlc_states(&status),
        [(ROYALTY, DlcState::Unavailable, None)]
    );

    fx.offer(RIMWORLD, "RimWorld", "2", &[file("start.sh", "s2")]);
    let pending = fx.pending(true, Options::default());
    assert!(
        pending
            .warnings()
            .contains(&format!("Royalty ({ROYALTY}) has no Linux installer.")),
        "{:?}",
        pending.warnings()
    );
    fx.apply_with(true, &pending, &Token::new(), |_| {})
        .unwrap();
    assert_eq!(
        fx.products(),
        [
            (RIMWORLD, ProductStatus::Installed, one("2")),
            (ROYALTY, ProductStatus::Unavailable, None),
        ]
    );
}

#[test]
fn an_update_adding_dlc_interrupted_is_finished_with_it() {
    for at in [
        "update.journal.after",
        "update.attempted.after",
        "update.commit.before",
    ] {
        let fx = Fixture::base_only();
        fx.offer(ROYALTY, "Royalty", "1", &[file("r", "royalty")]);
        fx.offer(IDEOLOGY, "Ideology", "1", &[file("i", "ideology")]);
        fx.recorded(ROYALTY, ProductStatus::Failed);
        fx.owns_dlc(&[RIMWORLD, ROYALTY, IDEOLOGY]);
        set_failpoint(Some(at));
        fx.update_err();
        let journal = fx.state().journal.unwrap();
        assert_eq!(journal.kind, JournalKind::Update, "{at}");
        assert_eq!(
            journal
                .target
                .products
                .iter()
                .map(|product| (product.id, product.status))
                .collect::<Vec<_>>(),
            [
                (RIMWORLD, ProductStatus::Installed),
                (ROYALTY, ProductStatus::Installed),
                (IDEOLOGY, ProductStatus::Installed),
            ],
            "{at}"
        );
        // Finished from the packages already downloaded, without GOG, and
        // a DLC whose package is gone then is never dropped.
        let pending = fx.pending(false, Options::default());
        assert!(pending.is_recovery() && pending.is_offline(), "{at}");
        fx.apply_with(false, &pending, &Token::new(), |_| {})
            .unwrap();
        assert_eq!(
            fx.products(),
            [
                (RIMWORLD, ProductStatus::Installed, one("1")),
                (ROYALTY, ProductStatus::Installed, one("1")),
                (IDEOLOGY, ProductStatus::Installed, one("1")),
            ],
            "{at}"
        );
        assert_eq!(
            (fx.read("r"), fx.read("i")),
            ("royalty".to_owned(), "ideology".to_owned())
        );
        assert_eq!(fx.state().journal, None);
    }
}

#[test]
fn a_recovery_never_drops_a_dlc_it_was_adding() {
    let fx = Fixture::base_only();
    fx.offer(ROYALTY, "Royalty", "1", &[file("r", "royalty")]);
    fx.owns_dlc(&[RIMWORLD, ROYALTY]);
    set_failpoint(Some("update.attempted.after"));
    fx.update_err();
    // The cached package is gone and GOG fails to serve it: the recovery
    // refuses rather than finishing without it.
    fx.clear_cache();
    fx.gog.lock().unwrap().failing = vec![ROYALTY];
    fx.try_update(Options::default()).unwrap_err();
    assert!(fx.state().journal.is_some());
    fx.gog.lock().unwrap().failing.clear();
    fx.update();
    assert_eq!(fx.read("r"), "royalty");
    assert_eq!(fx.state().journal, None);
}

#[test]
fn canceling_while_downloading_a_new_dlc_changes_nothing() {
    let fx = Fixture::base_only();
    fx.offer(ROYALTY, "Royalty", "1", &[file("r", "royalty")]);
    fx.owns_dlc(&[RIMWORLD, ROYALTY]);
    let before = (fx.state(), fx.tree());
    let pending = fx.pending(true, Options::default());
    let token = Token::new();
    let error = fx
        .apply_with(true, &pending, &token, |step| {
            if matches!(step, Progress::Download { product, .. } if product == ROYALTY) {
                token.cancel();
            }
        })
        .unwrap_err();
    assert!(
        matches!(
            error.downcast_ref(),
            Some(UpdateError::Canceled { journal: false })
        ),
        "{error:#}"
    );
    assert_eq!((fx.state(), fx.tree()), before);
}
