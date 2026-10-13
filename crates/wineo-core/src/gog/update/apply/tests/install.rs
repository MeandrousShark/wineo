//! Fresh native installs, which share an update's rebuild.

use super::*;
use crate::environment::Environment;
use crate::gog::update::install::{self, InstallError, Installed, Request};
use crate::gog::update::{UpdateState, check};

thread_local! {
    static BEFORE_CREATE: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

/// Runs a test's hook once, under the locks right before an install checks
/// its plan again.
pub(in crate::gog::update) fn before_create() {
    if let Some(hook) = BEFORE_CREATE.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
}

const BIOTECH: u64 = 1826322804;

fn request() -> Request {
    Request {
        name: "rimworld".to_owned(),
        gog_id: RIMWORLD,
        refresh: true,
        ..Request::default()
    }
}

impl Fixture {
    /// Nothing installed: no environment and no game folder.
    fn fresh() -> Self {
        let fx = Self::new();
        fs::remove_file(fx.paths.environment_config("rimworld")).unwrap();
        fs::remove_dir(&fx.folder).unwrap();
        fx
    }

    fn install_plan(&self, request: &Request) -> Result<install::Plan> {
        install::plan(&self.paths, &mut self.session(), request)
    }

    fn install_with(
        &self,
        plan: &install::Plan,
        token: &Token,
        progress: impl FnMut(Progress),
    ) -> Result<Installed> {
        install::apply(&self.paths, &mut self.session(), plan, token, progress)
    }

    fn try_install(&self) -> Result<Installed> {
        let plan = self.install_plan(&request())?;
        self.install_with(&plan, &Token::new(), |_| {})
    }

    fn install(&self) -> Installed {
        self.try_install().unwrap()
    }

    /// An install that stops at an injected failpoint.
    fn install_err(&self) {
        let error = self.try_install().unwrap_err();
        assert!(format!("{error:#}").contains("injected"), "{error:#}");
    }

    /// `wineo update rimworld`, finishing the interrupted install.
    fn finish(&self, online: bool) -> Updated {
        let pending = self.pending(online, Options::default());
        assert!(pending.is_recovery());
        assert_eq!(pending.kind(), JournalKind::Install);
        self.apply_with(online, &pending, &Token::new(), |_| {})
            .unwrap()
    }

    fn owns(&self, ids: &[u64]) {
        self.gog.lock().unwrap().owned = Some(ids.to_vec());
    }

    fn fail_download(&self, id: u64) {
        self.gog.lock().unwrap().failing.push(id);
    }

    fn language(&self, id: u64, language: &'static str) {
        self.gog
            .lock()
            .unwrap()
            .offers
            .get_mut(&id)
            .unwrap()
            .language = language;
    }

    /// Why a launch of the owner would be refused, if it would.
    fn launch_refusal(&self) -> Option<String> {
        folder::precheck(
            &self.paths,
            "rimworld",
            Some(&RIMWORLD.to_string()),
            &[self.folder.join("start.sh")],
        )
        .err()
        .map(|error| error.to_string())
    }

    fn statuses(&self) -> Vec<(u64, ProductStatus)> {
        self.state()
            .committed
            .unwrap()
            .products
            .iter()
            .map(|product| (product.id, product.status))
            .collect()
    }

    fn nothing_created(&self) {
        assert!(!self.folder.exists());
        assert!(!self.paths.environment_config("rimworld").exists());
        assert!(folder::peek(&self.paths, &self.folder).unwrap().is_none());
    }
}

/// RimWorld with Royalty and Ideology owned, and Biotech offered but not
/// owned.
fn offered() -> Fixture {
    let fx = Fixture::fresh();
    fx.offer(RIMWORLD, "RimWorld", "1.0", &base_v1());
    fx.offer(ROYALTY, "RimWorld - Royalty", "1.0", &royalty_v1());
    fx.offer(
        IDEOLOGY,
        "RimWorld - Ideology",
        "1.0",
        &[file("Data/Ideology/i.dat", "ideology v1")],
    );
    fx.offer(
        BIOTECH,
        "RimWorld - Biotech",
        "1.0",
        &[file("Data/Biotech/b.dat", "biotech v1")],
    );
    fx.owns(&[RIMWORLD, ROYALTY, IDEOLOGY]);
    fx
}

fn expected_v1() -> BTreeMap<String, String> {
    [
        ("Data", "/"),
        ("Data/Ideology", "/"),
        ("Data/Ideology/i.dat", "644 ideology v1"),
        ("Data/Royalty", "/"),
        ("Data/Royalty/r.dat", "644 royalty v1"),
        ("Data/big.dat", "644 big v1 big v1 big v1"),
        ("Data/core.dat", "644 core v1"),
        ("Data/old.dat", "644 old"),
        ("Data/old2.dat", "644 old2"),
        ("Manual", "-> docs/readme.txt"),
        ("docs", "/"),
        ("docs/readme.txt", "644 readme"),
        ("gameinfo", "644 RimWorld\n1.0\n"),
        ("kept", "/"),
        ("kept/readme", "644 dropped from a folder you use"),
        ("lib", "-> Data"),
        ("notes", "/"),
        ("notes/deep", "/"),
        (
            "notes/deep/readme",
            "644 inside a folder that becomes a file",
        ),
        ("notes/readme.txt", "644 a folder that becomes a file"),
        ("old", "/"),
        ("old/deep", "/"),
        ("old/deep/readme", "644 an obsolete folder"),
        ("shared.txt", "644 dlc shared"),
        ("start.sh", "755 v1 start"),
        ("support", "/"),
        (
            "support/postinst.sh",
            "755 #!/bin/bash\n# a stand-in for GOG's split file script\n",
        ),
        ("support/split_files", "644 Data/big.dat"),
    ]
    .into_iter()
    .map(|(path, content)| (path.to_owned(), content.to_owned()))
    .collect()
}

/// The folder holds the whole game, and its state is committed with every
/// product installed and nothing in flight.
fn assert_installed(fx: &Fixture) {
    assert_eq!(fx.tree(), expected_v1());
    let state = fx.state();
    assert_eq!(state.journal, None);
    assert_eq!(state.owner.as_deref(), Some("rimworld"));
    assert_eq!(
        fx.statuses(),
        [
            (RIMWORLD, ProductStatus::Installed),
            (ROYALTY, ProductStatus::Installed),
            (IDEOLOGY, ProductStatus::Installed),
        ]
    );
    let committed = state.committed.unwrap();
    assert_eq!(committed.language, "en");
    assert_eq!(committed.platform, Platform::Linux);
    assert_eq!(committed.track, Track::Installer);
    let files: BTreeMap<&str, &WinningEntry> = committed
        .files
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    // A later product wins a path both ship.
    assert_eq!(files["shared.txt"].product, ROYALTY);
    assert_eq!(files["start.sh"].product, RIMWORLD);
    assert!(fx.launch_refusal().is_none());
    assert!(fx.temporaries().is_empty());
}

#[test]
fn a_clean_install_with_dlc_creates_the_environment_and_commits_every_product() {
    let fx = offered();
    let plan = fx.install_plan(&request()).unwrap();
    assert_eq!(plan.environment(), "rimworld");
    assert_eq!(plan.title(), "RimWorld");
    assert_eq!(plan.folder(), fx.folder);
    assert_eq!(plan.language(), "en");
    assert!(!plan.resumes());
    assert_eq!(
        plan.products()
            .iter()
            .map(|product| (product.id, product.status, product.version.as_deref()))
            .collect::<Vec<_>>(),
        [
            (RIMWORLD, ProductStatus::Installed, Some("1.0")),
            (ROYALTY, ProductStatus::Installed, Some("1.0")),
            (IDEOLOGY, ProductStatus::Installed, Some("1.0")),
        ]
    );
    assert_eq!(plan.dlc_label(ROYALTY), "Royalty (1233017772)");
    // Planning changes nothing.
    fx.nothing_created();

    let installed = fx.install_with(&plan, &Token::new(), |_| {}).unwrap();
    assert_eq!(installed.environment, "rimworld");
    assert_eq!(installed.folder, fx.folder);
    assert!(!installed.resumed);
    assert!(installed.saved.is_empty() && installed.overwritten.is_empty());
    assert_installed(&fx);

    let environment = Environment::load(&fx.paths, "rimworld").unwrap();
    assert!(environment.config.is_native());
    assert_eq!(
        environment.config.gog_id.as_deref(),
        Some(RIMWORLD.to_string().as_str())
    );
    let target = &environment.config.targets["game"];
    assert!(target.default);
    assert_eq!(
        fx.paths.resolve_stored(&target.exe).unwrap().unwrap(),
        fx.folder.join("start.sh")
    );
    // The packages leave the cache once committed.
    for id in [RIMWORLD, ROYALTY, IDEOLOGY] {
        assert_eq!(cached(&fx, id), 0);
    }
    // Biotech isn't owned, so nothing of it is planned or fetched.
    assert_eq!(fx.downloads(), 3);
    assert!(!fx.folder.join("Data/Biotech").exists());
}

#[test]
fn listed_sizes_are_the_rounded_estimates() {
    let fx = offered();
    fx.pad(RIMWORLD);
    let plan = fx.install_plan(&request()).unwrap();
    let listed_sizes: Vec<u64> = [RIMWORLD, ROYALTY, IDEOLOGY]
        .iter()
        .map(|id| listed(fx.package_size(*id) as usize) as u64)
        .collect();
    assert_eq!(
        plan.products()
            .iter()
            .map(|product| product.download_size)
            .collect::<Vec<_>>(),
        listed_sizes
    );
    let total: u64 = listed_sizes.iter().sum();
    assert_eq!(plan.download_size(), total);
    assert_eq!(plan.installed_estimate(), total);
    // GOG lists the padded package smaller than it is; the download learns
    // its exact size, which the journal records.
    assert!(plan.products()[0].download_size < fx.package_size(RIMWORLD));
    set_failpoint(Some("install.state.after"));
    fx.install_with(&plan, &Token::new(), |_| {}).unwrap_err();
    let journal = fx.state().journal.unwrap();
    assert_eq!(journal.packages[0].size, fx.package_size(RIMWORLD));
    fx.finish(false);
}

#[test]
fn a_language_gog_doesnt_offer_falls_back_to_english() {
    let fx = offered();
    fx.language(RIMWORLD, "fr");
    let plan = fx
        .install_plan(&Request {
            language: Some("fr-FR".to_owned()),
            ..request()
        })
        .unwrap();
    assert_eq!(plan.requested_language(), "fr-FR");
    assert_eq!(plan.language(), "fr");
    assert!(plan.warnings().is_empty());
    let royalty = &plan.products()[1];
    assert_eq!(royalty.language.as_deref(), Some("en"));
    assert_eq!(
        royalty.note.as_deref(),
        Some("Royalty (1233017772) has no Linux installer in fr-FR, so it installs in English.")
    );

    // The base game in English only.
    let fx = offered();
    let plan = fx
        .install_plan(&Request {
            language: Some("de-AT".to_owned()),
            ..request()
        })
        .unwrap();
    assert_eq!(plan.language(), "en");
    assert_eq!(
        plan.warnings(),
        ["RimWorld has no Linux installer in de-AT, so it installs in English."]
    );
    fx.install_with(&plan, &Token::new(), |_| {}).unwrap();
    assert_eq!(fx.state().committed.unwrap().language, "en");
}

#[test]
fn the_global_language_is_used_when_none_is_asked_for() {
    let fx = offered();
    fx.language(RIMWORLD, "de");
    let config = fx.paths.global_config();
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "[gog]\nlanguage = \"de-DE\"\n").unwrap();
    let plan = fx.install_plan(&request()).unwrap();
    assert_eq!(plan.requested_language(), "de-DE");
    assert_eq!(plan.language(), "de");
}

#[test]
fn a_game_with_no_linux_installer_is_refused() {
    let fx = offered();
    fx.withdraw(RIMWORLD);
    let error = fx.install_plan(&request()).unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(InstallError::NoInstaller { .. })
    ));
    assert_eq!(
        error.to_string(),
        "RimWorld can't be installed: RimWorld has no Linux installer."
    );
}

#[test]
fn a_non_empty_folder_and_a_taken_name_are_refused() {
    let fx = offered();
    fx.write("mine.txt", "mine");
    let error = fx.install_plan(&request()).unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(FolderError::NotEmpty { .. })
    ));

    let fx = offered();
    fx.config("rimworld", Path::new("/elsewhere/start.sh"));
    let error = fx.install_plan(&request()).unwrap_err();
    assert!(
        matches!(
            error.downcast_ref(),
            Some(crate::Error::EnvironmentExists(_))
        ),
        "{error:#}"
    );
}

#[test]
fn a_folder_that_fills_up_after_planning_is_refused_under_the_lock() {
    let fx = offered();
    let plan = fx.install_plan(&request()).unwrap();
    let folder = fx.folder.clone();
    BEFORE_CREATE.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            fs::create_dir_all(&folder).unwrap();
            fs::write(folder.join("mine.txt"), "mine").unwrap();
        }))
    });
    let error = fx.install_with(&plan, &Token::new(), |_| {}).unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(FolderError::NotEmpty { .. })
    ));
    assert!(!fx.paths.environment_config("rimworld").exists());
    assert!(folder::peek(&fx.paths, &fx.folder).unwrap().is_none());
}

#[test]
fn a_folder_inside_another_game_is_refused() {
    let fx = offered();
    let other = fx.folder.parent().unwrap().to_owned();
    fs::create_dir_all(&other).unwrap();
    FolderLock::exclusive(&fx.paths, &other, &|| false)
        .unwrap()
        .write(&FolderState::new(other.clone(), Some("other".to_owned())))
        .unwrap();
    let error = fx.install_plan(&request()).unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(FolderError::Overlap { .. })
    ));
}

#[test]
fn installing_again_carries_on_the_environments_own_interrupted_install() {
    let fx = offered();
    set_failpoint_nth("update.publish.before", 3);
    fx.install_err();
    assert!(fs::read_dir(&fx.folder).unwrap().next().is_some());
    let plan = fx.install_plan(&request()).unwrap();
    assert!(plan.resumes());
    let installed = fx.install_with(&plan, &Token::new(), |_| {}).unwrap();
    assert!(installed.resumed);
    assert_installed(&fx);

    // Another environment's interrupted install is somebody else's folder.
    let fx = offered();
    set_failpoint_nth("update.publish.before", 3);
    fx.install_err();
    let error = fx
        .install_plan(&Request {
            name: "rimworld-2".to_owned(),
            ..request()
        })
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(FolderError::NotEmpty { .. })
    ));
}

#[test]
fn an_empty_folder_with_another_environments_state_is_refused() {
    let fx = offered();
    fs::create_dir_all(&fx.folder).unwrap();
    for journal in [false, true] {
        let mut state = FolderState::new(fx.folder.clone(), Some("other".to_owned()));
        if journal {
            state.journal = Some(Journal {
                operation: "op".to_owned(),
                kind: JournalKind::Update,
                target: Target {
                    gog_id: RIMWORLD,
                    platform: Platform::Linux,
                    language: "en".to_owned(),
                    track: Track::Installer,
                    products: Vec::new(),
                },
                packages: Vec::new(),
                attempted: Vec::new(),
            });
        }
        fx.save(&state);
        // This game's environment doesn't take it over as its own install.
        fx.config("rimworld", Path::new("/elsewhere/start.sh"));
        let error = fx.install_plan(&request()).unwrap_err();
        assert!(
            matches!(
                error.downcast_ref(),
                Some(crate::Error::EnvironmentExists(_))
            ),
            "{error:#}"
        );
        fs::remove_file(fx.paths.environment_config("rimworld")).unwrap();
        let error = fx.install_plan(&request()).unwrap_err();
        assert!(
            matches!(error.downcast_ref(), Some(FolderError::OwnedByOther { .. })),
            "{error:#}"
        );
        assert_eq!(fx.state(), state);
    }
}

#[test]
fn a_dlc_in_another_language_than_the_game_cant_be_compared_afterwards() {
    let fx = offered();
    fx.language(RIMWORLD, "fr");
    let plan = fx
        .install_plan(&Request {
            language: Some("fr-FR".to_owned()),
            ..request()
        })
        .unwrap();
    fx.install_with(&plan, &Token::new(), |_| {}).unwrap();
    assert_eq!(fx.state().committed.unwrap().language, "fr");
    // The folder records one language, so the English DLC isn't found in
    // it: this pins down today's behavior, an open question for the plan.
    let status = check(&mut fx.session(), "rimworld", true).unwrap();
    assert_eq!(status.state, UpdateState::CantCompare);
    assert_eq!(
        status.reason.map(|reason| reason.as_str().to_owned()),
        Some("Royalty (1233017772) has no Linux installer in fr.".to_owned())
    );
}

#[test]
fn a_dlc_with_no_linux_installer_is_recorded_unavailable() {
    let fx = offered();
    fx.withdraw(IDEOLOGY);
    let plan = fx.install_plan(&request()).unwrap();
    let ideology = &plan.products()[2];
    assert_eq!(ideology.status, ProductStatus::Unavailable);
    assert_eq!(ideology.download_size, 0);
    assert_eq!(
        ideology.note.as_deref(),
        Some("Ideology (1827857764) has no Linux installer.")
    );
    fx.install_with(&plan, &Token::new(), |_| {}).unwrap();
    assert_eq!(
        fx.statuses(),
        [
            (RIMWORLD, ProductStatus::Installed),
            (ROYALTY, ProductStatus::Installed),
            (IDEOLOGY, ProductStatus::Unavailable),
        ]
    );
    assert!(!fx.folder.join("Data/Ideology").exists());
    assert!(fx.launch_refusal().is_none());
}

#[test]
fn a_dlc_failing_before_changing_files_leaves_the_game_launchable() {
    let fx = offered();
    fx.fail_download(ROYALTY);
    // A package that isn't a GOG package can't be opened.
    fx.gog
        .lock()
        .unwrap()
        .offers
        .get_mut(&IDEOLOGY)
        .unwrap()
        .bytes = Some(b"#!/bin/sh\nnot a zip\n".to_vec());
    let installed = fx.install();
    assert_eq!(
        installed
            .products
            .iter()
            .map(|product| (product.id, product.status))
            .collect::<Vec<_>>(),
        [
            (RIMWORLD, ProductStatus::Installed),
            (ROYALTY, ProductStatus::Failed),
            (IDEOLOGY, ProductStatus::Failed),
        ]
    );
    for (dlc, why) in [
        ("Royalty (1233017772)", "Couldn't download"),
        ("Ideology (1827857764)", "Couldn't open"),
    ] {
        let prefix = format!("{dlc} failed and wasn't installed: {why}");
        assert!(
            installed
                .warnings
                .iter()
                .any(|warning| warning.starts_with(&prefix)),
            "{:?}",
            installed.warnings
        );
    }
    assert_eq!(fx.state().journal, None);
    assert_eq!(fx.read("shared.txt"), "base shared");
    assert!(!fx.folder.join("Data/Royalty").exists());
    assert!(fx.launch_refusal().is_none());
}

#[test]
fn a_base_game_failing_to_download_changes_nothing() {
    let fx = offered();
    fx.fail_download(RIMWORLD);
    let error = fx.try_install().unwrap_err();
    assert!(
        format!("{error:#}").contains("Couldn't download RimWorld"),
        "{error:#}"
    );
    fx.nothing_created();
}

#[test]
fn a_dlc_failing_partway_blocks_the_game_until_update_finishes_it() {
    let fx = offered();
    // Data, Data/Ideology, then Ideology's file.
    set_failpoint_nth("update.publish.before", 2);
    fx.install_err();
    let state = fx.state();
    assert_eq!(state.journal.unwrap().kind, JournalKind::Install);
    assert_eq!(state.committed, None);
    assert!(fx.folder.join("Data/Ideology").is_dir());
    assert!(!fx.folder.join("Data/Ideology/i.dat").exists());
    assert_eq!(
        fx.launch_refusal().as_deref(),
        Some("rimworld's game folder is mid-update. Finish it with wineo update rimworld.")
    );

    let downloads = fx.downloads();
    let updated = fx.finish(false);
    assert!(updated.recovered);
    assert_eq!(updated.kind, JournalKind::Install);
    assert_eq!(fx.downloads(), downloads);
    assert_installed(&fx);
}

#[test]
fn an_install_stopped_after_downloading_installs_again_without_downloading() {
    let fx = offered();
    set_failpoint(Some("install.create.before"));
    fx.install_err();
    fx.nothing_created();
    let downloads = fx.downloads();
    assert_eq!(downloads, 3);
    fx.install();
    assert_eq!(fx.downloads(), downloads);
    assert_installed(&fx);
}

#[test]
fn a_crash_between_the_config_and_the_state_leaves_a_lost_state() {
    let fx = offered();
    set_failpoint(Some("install.config.after"));
    fx.install_err();
    assert!(Environment::load(&fx.paths, "rimworld").is_ok());
    assert!(folder::peek(&fx.paths, &fx.folder).unwrap().is_none());
    let error = fx.plan_with(false, Options::default()).unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(FolderError::StateMissing { .. })
    ));
}

#[test]
fn an_install_stopped_after_its_state_is_written_is_finished_by_update() {
    let fx = offered();
    set_failpoint(Some("install.state.after"));
    fx.install_err();
    let state = fx.state();
    assert_eq!(state.owner.as_deref(), Some("rimworld"));
    assert_eq!(state.committed, None);
    let journal = state.journal.unwrap();
    assert_eq!(journal.kind, JournalKind::Install);
    assert_eq!(journal.packages.len(), 3);
    assert!(journal.attempted.is_empty());
    assert_eq!(fs::read_dir(&fx.folder).unwrap().count(), 0);
    assert!(fx.launch_refusal().is_some());

    // `update --check` and `validate` report it like an unfinished update.
    let status = check(&mut fx.session(), "rimworld", true).unwrap();
    assert!(status.unfinished);
    assert_eq!(status.state, UpdateState::CantCompare);
    let report = crate::validate::check_environment(&fx.paths, "rimworld").unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.code == crate::validate::Code::GogMidUpdate),
        "{:?}",
        report.problems
    );

    let downloads = fx.downloads();
    fx.finish(false);
    assert_eq!(fx.downloads(), downloads);
    assert_installed(&fx);
}

#[test]
fn an_install_interrupted_at_any_write_is_finished_by_update() {
    for (point, skip) in [
        ("update.journal.before", 0),
        ("update.journal.after", 0),
        ("update.attempted.before", 0),
        ("update.attempted.after", 0),
        ("update.publish.before", 0),
        ("update.publish.temp", 4),
        ("update.publish.after", 10),
        ("update.commit.before", 0),
    ] {
        let fx = offered();
        set_failpoint_nth(point, skip);
        fx.install_err();
        assert!(!failpoint_armed(), "{point}");
        assert_eq!(
            fx.state().journal.map(|journal| journal.kind),
            Some(JournalKind::Install),
            "{point}"
        );
        assert!(fx.launch_refusal().is_some(), "{point}");
        fx.finish(false);
        assert_installed(&fx);
    }
}

#[test]
fn an_interrupted_install_with_its_packages_gone_downloads_them_again() {
    let fx = offered();
    set_failpoint_nth("update.publish.before", 3);
    fx.install_err();
    fx.clear_cache();
    let error = fx.plan_with(false, Options::default()).unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(AccountError::LoggedOut)
    ));
    let downloads = fx.downloads();
    fx.finish(true);
    assert_eq!(fx.downloads(), downloads + 3);
    assert_installed(&fx);
}

#[test]
fn canceling_while_downloading_changes_nothing() {
    let fx = offered();
    let plan = fx.install_plan(&request()).unwrap();
    let token = Token::new();
    let error = fx
        .install_with(&plan, &token, |progress| {
            if matches!(progress, Progress::Download { .. }) {
                token.cancel();
            }
        })
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(UpdateError::Canceled { journal: false })
    ));
    fx.nothing_created();
}

#[test]
fn canceling_at_the_last_check_before_creating_changes_nothing() {
    let fx = offered();
    let plan = fx.install_plan(&request()).unwrap();
    let token = Token::new();
    let canceler = token.clone();
    BEFORE_CREATE.with(|hook| *hook.borrow_mut() = Some(Box::new(move || canceler.cancel())));
    let error = fx.install_with(&plan, &token, |_| {}).unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(UpdateError::Canceled { journal: false })
    ));
    fx.nothing_created();
    // Downloaded already, so installing again fetches nothing.
    let downloads = fx.downloads();
    fx.install();
    assert_eq!(fx.downloads(), downloads);
}

#[test]
fn canceling_while_staging_or_publishing_keeps_the_journal_for_update() {
    for stop_at in ["stage", "publish"] {
        let fx = offered();
        let plan = fx.install_plan(&request()).unwrap();
        let token = Token::new();
        let error = fx
            .install_with(&plan, &token, |progress| match progress {
                Progress::Stage { .. } if stop_at == "stage" => token.cancel(),
                Progress::Publish { done: 2, .. } if stop_at == "publish" => token.cancel(),
                _ => {}
            })
            .unwrap_err();
        assert!(
            matches!(
                error.downcast_ref(),
                Some(UpdateError::Canceled { journal: true })
            ),
            "{stop_at}: {error:#}"
        );
        assert!(fx.launch_refusal().is_some(), "{stop_at}");
        fx.finish(false);
        assert_installed(&fx);
    }
}

#[test]
fn not_enough_space_is_refused_before_downloading() {
    let fx = offered();
    let plan = fx.install_plan(&request()).unwrap();
    FREE.with(|free| free.set(Some(10)));
    let error = fx.install_with(&plan, &Token::new(), |_| {}).unwrap_err();
    FREE.with(|free| free.set(None));
    assert!(matches!(
        error.downcast_ref(),
        Some(PackageError::NoSpace { .. })
    ));
    // The cache shares the folder's filesystem here, so the download and the
    // estimate are checked together first.
    assert_eq!(fx.downloads(), 0);
    fx.nothing_created();
}

#[test]
fn a_portable_root_is_refused() {
    let fx = offered();
    let home = fx.root.path().to_owned();
    let portable = Paths::select(
        move |key| (key == "HOME").then(|| home.clone().into_os_string()),
        Some(&fx.root.path().join("usb")),
    )
    .unwrap();
    let mut session = Session::open(&portable, &*fx.keyring, client_at(&fx.server.base))
        .unwrap()
        .unwrap();
    let error = install::plan(&portable, &mut session, &request()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "GOG games live in the host library. Run this without --portable."
    );
}

#[test]
fn a_dry_run_plan_writes_nothing_but_a_renewed_login() {
    let fx = offered();
    let mut session = fx.session();
    let dry = Request {
        read_only: true,
        ..request()
    };
    install::plan(&fx.paths, &mut session, &dry).unwrap();
    let before = crate::test_snapshot::snapshot_tree(fx.root.path());
    install::plan(&fx.paths, &mut session, &dry).unwrap();
    crate::test_snapshot::assert_unchanged(
        &before,
        &crate::test_snapshot::snapshot_tree(fx.root.path()),
    );
    assert!(!fx.paths.gog_product_cache(RIMWORLD).exists());
    assert!(!fx.paths.gog_owned_cache(USER).exists());
}
