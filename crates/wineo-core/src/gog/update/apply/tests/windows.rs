//! Windows installs: GOG's Inno installers, run by a fake Wine in a new
//! environment's prefix.

use super::*;
use crate::config::GlobalConfig;
use crate::environment::Environment;
use crate::gog::update::install::{self, Installed, Request};
use crate::gog::update::windows::tests::{set_before_target, set_graphical};
use crate::gog::update::windows::{Event, SetupError};
use crate::operation::{Locks, Operation};
use crate::test_script::write_script;

/// A DLC of the game, which only has a Windows installer.
const DLC: u64 = ROYALTY;

fn request() -> Request {
    Request {
        name: "rimworld".to_owned(),
        gog_id: RIMWORLD,
        refresh: true,
        runner: Some("fake".to_owned()),
        ..Request::default()
    }
}

/// A `goggame-<id>.info` like GOG's installers write.
fn info(id: u64, languages: &[&str], language: &str, tasks: serde_json::Value) -> String {
    serde_json::json!({
        "gameId": id.to_string(),
        "rootGameId": RIMWORLD.to_string(),
        "name": "RimWorld",
        "language": language,
        "languages": languages,
        "playTasks": tasks,
        // The info file's own format, not the game's version.
        "version": 1,
    })
    .to_string()
}

fn primary_task() -> serde_json::Value {
    serde_json::json!([
        {"type": "URLTask", "isPrimary": false, "link": "https://www.gog.com/forum"},
        {
            "type": "FileTask",
            "isPrimary": true,
            "path": "bin\\Game.exe",
            "workingDir": "bin",
            "arguments": "-windowed",
        },
    ])
}

fn english(id: u64) -> String {
    info(id, &["en-US"], "English", primary_task())
}

impl Fixture {
    /// Nothing installed, and a fake Wine runner that runs setup programs.
    fn windows() -> Self {
        let fx = Self::new();
        fs::remove_file(fx.paths.environment_config("rimworld")).unwrap();
        fs::remove_dir(&fx.folder).unwrap();
        fs::create_dir_all(fx.runs()).unwrap();
        let bin = GlobalConfig::default()
            .runner_root(&fx.paths)
            .unwrap()
            .join("fake/bin");
        fs::create_dir_all(&bin).unwrap();
        write_script(bin.join("wineserver"), "#!/bin/sh\nexit 0\n");
        // A setup program is a script it sources with `$exe`, `$DIR`, and
        // `$RUNS` set, after recording how it was run.
        write_script(
            bin.join("wine"),
            format!(
                r#"#!/bin/sh
RUNS='{runs}'
case "$1" in
wineboot) [ "$2" = --init ] && echo 'WINE REGISTRY Version 2' > "$WINEPREFIX/system.reg"; exit 0 ;;
*.exe) ;;
*) exit 0 ;;
esac
exe="$1"
shift
name=$(basename "$exe")
{{ printf '%s\n' "$exe" "$@"; printf 'LANG=%s\nLC_ALL=%s\n' "$LANG" "$LC_ALL"; }} > "$RUNS/$name.args"
echo "$name" >> "$RUNS/log"
DIR=
for arg in "$@"; do
    case "$arg" in /DIR=Z:*) DIR=$(printf '%s' "${{arg#/DIR=Z:}}" | tr '\\' '/') ;; esac
done
. "$exe"
"#,
                runs = fx.runs().display()
            ),
        );
        fx
    }

    fn runs(&self) -> PathBuf {
        self.root.path().join("runs")
    }

    /// Offers a Windows installer for `id`: a setup program and one part.
    /// Run, it checks the part is beside it, then exits with the code in
    /// `runs/exit-<id>` if there is one, writing nothing for the codes that
    /// mean it never started, else `files` and `info` as `goggame-<id>.info`.
    fn offer_windows(
        &self,
        id: u64,
        title: &'static str,
        version: &str,
        files: &[(&str, &str)],
        info: Option<String>,
    ) {
        let stem = format!("setup_{id}_{}", version.replace(['.', ' '], "_"));
        let part = format!("{stem}-1.bin");
        let mut script = format!(
            "[ -f \"$(dirname \"$exe\")/{part}\" ] || exit 99\n\
             code=$(cat \"$RUNS/exit-{id}\" 2>/dev/null || echo 0)\n\
             case \"$code\" in 1|2|3|7) exit \"$code\" ;; esac\n\
             mkdir -p \"$DIR\"\n"
        );
        for (path, content) in files {
            script.push_str(&format!(
                "mkdir -p \"$(dirname \"$DIR/{path}\")\" && printf '%s' '{content}' > \"$DIR/{path}\"\n"
            ));
        }
        if let Some(info) = info {
            script.push_str(&format!(
                "cat > \"$DIR/goggame-{id}.info\" <<'INFO'\n{info}\nINFO\n"
            ));
        }
        script.push_str("exit \"$code\"\n");
        let mut gog = self.gog.lock().unwrap();
        gog.offers.entry(id).or_insert_with(|| Offer {
            title,
            version: version.to_owned(),
            bytes: None,
            checksum: true,
            language: "en",
            also: Vec::new(),
        });
        if id != RIMWORLD && !gog.dlc.contains(&id) {
            gog.dlc.push(id);
        }
        gog.windows.insert(
            id,
            WindowsOffer {
                version: version.to_owned(),
                languages: vec!["en"],
                files: vec![
                    (format!("{stem}.exe"), script.into_bytes()),
                    (part, b"the rest of the installer".to_vec()),
                ],
            },
        );
    }

    fn windows_languages(&self, id: u64, languages: &[&'static str]) {
        self.gog
            .lock()
            .unwrap()
            .windows
            .get_mut(&id)
            .unwrap()
            .languages = languages.to_vec();
    }

    fn exits(&self, id: u64, code: i32) {
        fs::write(self.runs().join(format!("exit-{id}")), code.to_string()).unwrap();
    }

    /// The arguments the last run of `id`'s setup program got, its path
    /// first, then its locale.
    fn args(&self, id: u64) -> Vec<String> {
        let found = fs::read_dir(self.runs())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .find(|name| name.starts_with(&format!("setup_{id}_")) && name.ends_with(".args"))
            .unwrap();
        fs::read_to_string(self.runs().join(found))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Every setup program run, in order.
    fn ran(&self) -> Vec<String> {
        fs::read_to_string(self.runs().join("log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn plan_windows(&self, request: &Request) -> Result<install::Plan> {
        install::plan(&self.paths, &mut self.session(), request, &Token::new())
    }

    fn install_windows(&self, request: &Request) -> Result<Installed> {
        let plan = self.plan_windows(request)?;
        install::apply(
            &self.paths,
            &mut self.session(),
            &plan,
            &Token::new(),
            |_| {},
        )
    }

    /// `wineo update rimworld`, finishing the interrupted install.
    fn finish_windows(&self) -> Updated {
        let pending = self.pending(true, Options::default());
        assert!(pending.is_recovery());
        assert_eq!(pending.kind(), JournalKind::Install);
        self.apply_with(true, &pending, &Token::new(), |_| {})
            .unwrap()
    }

    fn statuses_windows(&self) -> Vec<(u64, ProductStatus)> {
        self.state()
            .committed
            .unwrap()
            .products
            .iter()
            .map(|product| (product.id, product.status))
            .collect()
    }

    fn launch_refused(&self) -> bool {
        folder::precheck(
            &self.paths,
            "rimworld",
            Some(&RIMWORLD.to_string()),
            &[self.folder.join("bin/Game.exe")],
        )
        .is_err()
    }
}

/// RimWorld and one DLC, both only on Windows.
fn offered() -> Fixture {
    let fx = Fixture::windows();
    fx.offer_windows(
        RIMWORLD,
        "RimWorld",
        "1.0",
        &[("bin/Game.exe", "game"), ("Data/core.dat", "core v1")],
        Some(english(RIMWORLD)),
    );
    fx.offer_windows(
        DLC,
        "RimWorld - Royalty",
        "1.0",
        &[("Data/Royalty/r.dat", "royalty v1")],
        Some(info(DLC, &["en-US"], "English", serde_json::json!([]))),
    );
    fx.owns_ids(&[RIMWORLD, DLC]);
    fx
}

impl Fixture {
    fn owns_ids(&self, ids: &[u64]) {
        self.gog.lock().unwrap().owned = Some(ids.to_vec());
    }
}

/// The game and its DLC are installed and committed, with nothing in
/// flight and the play task as the default target.
fn assert_installed(fx: &Fixture) {
    let state = fx.state();
    assert_eq!(state.journal, None);
    assert_eq!(state.owner.as_deref(), Some("rimworld"));
    let committed = state.committed.unwrap();
    assert_eq!(committed.platform, Platform::Windows);
    assert_eq!(committed.track, Track::Installer);
    assert_eq!(committed.language, "en");
    assert!(committed.files.is_empty());
    assert_eq!(
        fx.statuses_windows(),
        [
            (RIMWORLD, ProductStatus::Installed),
            (DLC, ProductStatus::Installed)
        ]
    );
    // The version recorded is the installer's that ran, which updates
    // compare with; the info file has none.
    assert!(
        committed
            .products
            .iter()
            .all(|product| product.version.as_deref() == Some("1.0")
                && product.reported_language.is_none())
    );
    assert_eq!(fx.read("Data/core.dat"), "core v1");
    assert_eq!(fx.read("Data/Royalty/r.dat"), "royalty v1");
    let environment = Environment::load(&fx.paths, "rimworld").unwrap();
    assert!(!environment.config.is_native());
    assert_eq!(
        environment.config.gog_id.as_deref(),
        Some(RIMWORLD.to_string().as_str())
    );
    let target = &environment.config.targets["game"];
    assert!(target.default);
    assert_eq!(
        fx.paths.resolve_stored(&target.exe).unwrap().unwrap(),
        fx.folder.join("bin/Game.exe")
    );
    assert_eq!(
        target.cwd.as_deref().map(PathBuf::from),
        Some(fx.folder.join("bin"))
    );
    assert_eq!(target.args, ["-windowed"]);
    assert!(!fx.launch_refused());
}

#[test]
fn a_windows_install_creates_the_environment_runs_each_installer_and_commits() {
    let fx = offered();
    let plan = fx.plan_windows(&request()).unwrap();
    assert_eq!(plan.platform(), Platform::Windows);
    assert_eq!(plan.creation().unwrap().runner().name(), "fake");
    assert_eq!(
        plan.products()
            .iter()
            .map(|product| (product.id, product.status))
            .collect::<Vec<_>>(),
        [
            (RIMWORLD, ProductStatus::Installed),
            (DLC, ProductStatus::Installed)
        ]
    );
    // Windows installers don't list what they unpack to.
    assert_eq!(plan.installed_estimate(), 2 * plan.download_size());
    assert!(!fx.folder.exists());
    assert!(!fx.paths.environment_config("rimworld").exists());

    let mut events = Vec::new();
    let installed = install::apply_with_events(
        &fx.paths,
        &mut fx.session(),
        &plan,
        &Token::new(),
        |_| {},
        |event| {
            if let Event::Installing {
                product,
                interactive,
            } = event
            {
                events.push((product, interactive));
            }
        },
    )
    .unwrap();
    assert_eq!(events, [(RIMWORLD, false), (DLC, false)]);
    assert_eq!(installed.target, Some(fx.folder.join("bin/Game.exe")));
    assert_eq!(installed.reported_language.as_deref(), Some("en"));
    assert!(installed.warnings.is_empty(), "{:?}", installed.warnings);
    assert_installed(&fx);

    // Inno's flags, the folder as a Wine path, the language, and the
    // locale it falls back to.
    let args = fx.args(RIMWORLD);
    let log = fx
        .paths
        .runtime_dir("rimworld")
        .join(format!("gog-setup-{RIMWORLD}.log"));
    let z = |path: &Path| format!("Z:{}", path.display().to_string().replace('/', "\\"));
    assert_eq!(
        args[1..],
        [
            "/SP-".to_owned(),
            "/SILENT".to_owned(),
            "/SUPPRESSMSGBOXES".to_owned(),
            "/NORESTART".to_owned(),
            "/NOCANCEL".to_owned(),
            format!("/LOG={}", z(&log)),
            format!("/DIR={}", z(&fx.folder)),
            "/LANG=english".to_owned(),
            "LANG=en_US.UTF-8".to_owned(),
            "LC_ALL=en_US.UTF-8".to_owned(),
        ]
    );
    // The setup program ran from the download cache, with its part beside
    // it under GOG's name.
    let exe = PathBuf::from(&args[0]);
    assert!(exe.starts_with(fx.paths.gog_download_cache()));
    assert!(exe.ends_with(format!("installer_windows_en/setup_{RIMWORLD}_1_0.exe")));
    // The packages leave the cache once committed.
    assert_eq!(cached(&fx, RIMWORLD), 0);
    assert_eq!(cached(&fx, DLC), 0);
}

#[test]
fn a_game_with_only_windows_installers_defaults_to_windows_and_linux_can_be_asked_for() {
    let fx = offered();
    fx.offer(RIMWORLD, "RimWorld", "1.0", &base_v1());
    // Natively when GOG has a Linux installer.
    assert_eq!(
        fx.plan_windows(&request()).unwrap().platform(),
        Platform::Linux
    );
    let plan = fx
        .plan_windows(&Request {
            platform: Some(Platform::Windows),
            ..request()
        })
        .unwrap();
    assert_eq!(plan.platform(), Platform::Windows);
}

#[test]
fn a_play_task_with_no_file_or_no_info_gives_no_default_target() {
    for info_file in [
        Some(info(
            RIMWORLD,
            &["en-US"],
            "English",
            serde_json::json!([{"type": "URLTask", "isPrimary": true, "link": "https://example.com"}]),
        )),
        None,
    ] {
        let fx = Fixture::windows();
        fx.offer_windows(
            RIMWORLD,
            "RimWorld",
            "1.0",
            &[("Data/core.dat", "core v1")],
            info_file.clone(),
        );
        fx.owns_ids(&[RIMWORLD]);
        let installed = fx.install_windows(&request()).unwrap();
        assert_eq!(installed.target, None);
        let environment = Environment::load(&fx.paths, "rimworld").unwrap();
        assert!(environment.config.targets.is_empty());
        assert_eq!(fx.state().journal, None);
        assert_eq!(
            installed.reported_language,
            info_file.as_ref().map(|_| "en".to_owned())
        );
    }
}

#[test]
fn a_language_the_installed_game_doesnt_report_is_said_without_guessing_why() {
    let fx = Fixture::windows();
    fx.offer_windows(
        RIMWORLD,
        "RimWorld",
        "1.0",
        &[("bin/Game.exe", "game")],
        Some(info(RIMWORLD, &["fr-FR"], "French", primary_task())),
    );
    fx.owns_ids(&[RIMWORLD]);
    let installed = fx.install_windows(&request()).unwrap();
    assert_eq!(
        installed.warnings,
        ["Requested English; the installed game reports French."]
    );
    assert_eq!(installed.reported_language.as_deref(), Some("fr"));
    // The state records the installer's language, and what the game
    // reports instead.
    let committed = fx.state().committed.unwrap();
    assert_eq!(committed.language, "en");
    assert_eq!(
        committed.products[0].reported_language.as_deref(),
        Some("fr")
    );
}

#[test]
fn another_language_picks_its_installer_inno_name_and_locale() {
    let fx = Fixture::windows();
    fx.offer_windows(
        RIMWORLD,
        "RimWorld",
        "1.0",
        &[("bin/Game.exe", "game")],
        Some(info(RIMWORLD, &["fr-FR"], "French", primary_task())),
    );
    fx.windows_languages(RIMWORLD, &["en", "fr"]);
    fx.owns_ids(&[RIMWORLD]);
    let plan = fx
        .plan_windows(&Request {
            language: Some("fr-FR".to_owned()),
            ..request()
        })
        .unwrap();
    assert_eq!(plan.language(), "fr");
    let installed =
        install::apply(&fx.paths, &mut fx.session(), &plan, &Token::new(), |_| {}).unwrap();
    assert!(installed.warnings.is_empty(), "{:?}", installed.warnings);
    let args = fx.args(RIMWORLD);
    assert!(args[0].contains("installer_windows_fr"), "{args:?}");
    assert!(args.contains(&"/LANG=french".to_owned()), "{args:?}");
    assert!(args.contains(&"LC_ALL=fr_FR.UTF-8".to_owned()), "{args:?}");
    assert_eq!(fx.state().committed.unwrap().language, "fr");
}

#[test]
fn without_a_graphical_session_nothing_is_downloaded() {
    let fx = offered();
    set_graphical(false);
    let error = fx.plan_windows(&request()).unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(SetupError::NoGraphicalSession)
    ));

    // Planned with one, applied without.
    set_graphical(true);
    let plan = fx.plan_windows(&request()).unwrap();
    set_graphical(false);
    let error =
        install::apply(&fx.paths, &mut fx.session(), &plan, &Token::new(), |_| {}).unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(SetupError::NoGraphicalSession)
    ));
    assert_eq!(fx.downloads(), 0);
    assert!(!fx.folder.exists());
}

#[test]
fn the_space_check_is_twice_the_installers_and_says_its_an_estimate() {
    let fx = offered();
    let plan = fx.plan_windows(&request()).unwrap();
    FREE.with(|free| free.set(Some(plan.download_size() * 2)));
    let error =
        install::apply(&fx.paths, &mut fx.session(), &plan, &Token::new(), |_| {}).unwrap_err();
    FREE.with(|free| free.set(None));
    assert!(matches!(
        error.downcast_ref(),
        Some(SetupError::NoSpace { .. })
    ));
    assert!(error.to_string().contains("an estimate"), "{error}");
    assert_eq!(fx.downloads(), 0);
}

#[test]
fn a_setup_that_never_started_offers_its_wizard_once() {
    for code in [1, 3, 7] {
        let fx = offered();
        fx.exits(RIMWORLD, code);
        let error = fx.install_windows(&request()).unwrap_err();
        match error.downcast_ref() {
            Some(SetupError::NotStarted {
                product,
                code: found,
                wizard,
                ..
            }) => {
                assert_eq!((*product, *found, *wizard), (RIMWORLD, code, true));
            }
            _ => panic!("{error:#}"),
        }
        // The environment and the journal stay; nothing was installed.
        assert!(fx.paths.environment_config("rimworld").exists());
        assert_eq!(fx.state().journal.unwrap().kind, JournalKind::Install);
        assert!(fx.launch_refused());
        assert_eq!(fx.tree(), BTreeMap::new());

        // The installer's own wizard, without the flags that hide it.
        let wizard = Request {
            wizard: true,
            ..request()
        };
        let plan = fx.plan_windows(&wizard).unwrap();
        assert!(plan.resumes() && plan.wizard());
        let error =
            install::apply(&fx.paths, &mut fx.session(), &plan, &Token::new(), |_| {}).unwrap_err();
        // Stopped again: no wizard to offer this time.
        assert!(matches!(
            error.downcast_ref(),
            Some(SetupError::NotStarted { wizard: false, .. })
        ));
        let args = fx.args(RIMWORLD);
        assert!(!args.contains(&"/SILENT".to_owned()), "{args:?}");
        assert!(args.contains(&"/NORESTART".to_owned()), "{args:?}");

        fs::remove_file(fx.runs().join(format!("exit-{RIMWORLD}"))).unwrap();
        fx.install_windows(&wizard).unwrap();
        assert_installed(&fx);
    }
}

#[test]
fn a_setup_canceled_before_changing_anything_keeps_the_journal() {
    let fx = offered();
    fx.exits(RIMWORLD, 2);
    let error = fx.install_windows(&request()).unwrap_err();
    assert!(
        matches!(error.downcast_ref(), Some(SetupError::Canceled { product, .. }) if *product == RIMWORLD),
        "{error:#}"
    );
    assert_eq!(fx.state().journal.unwrap().kind, JournalKind::Install);
    assert!(fx.launch_refused());
    fs::remove_file(fx.runs().join(format!("exit-{RIMWORLD}"))).unwrap();
    fx.finish_windows();
    assert_installed(&fx);
}

#[test]
fn a_setup_that_stopped_partway_keeps_every_file_until_update_finishes_it() {
    for code in [4, 5, 6, 8, 9] {
        let fx = offered();
        fx.exits(RIMWORLD, code);
        let error = fx.install_windows(&request()).unwrap_err();
        match error.downcast_ref() {
            Some(SetupError::Partway {
                product, detail, ..
            }) => {
                assert_eq!(*product, RIMWORLD);
                assert_eq!(*detail, format!("exit code {code}"));
            }
            _ => panic!("{error:#}"),
        }
        assert_eq!(
            error.to_string(),
            format!(
                "RimWorld's installer stopped partway (exit code {code}). Finish it with wineo update rimworld."
            )
        );
        let state = fx.state();
        let journal = state.journal.unwrap();
        assert_eq!(journal.kind, JournalKind::Install);
        assert_eq!(journal.attempted.len(), 1);
        assert_eq!(journal.attempted[0].path, ".");
        assert_eq!(journal.packages.len(), 4);
        assert!(fx.launch_refused());
        // What it wrote stays, and so does anything of yours.
        assert_eq!(fx.read("Data/core.dat"), "core v1");
        fx.write("saves/mine.sav", "mine");

        fs::remove_file(fx.runs().join(format!("exit-{RIMWORLD}"))).unwrap();
        let downloads = fx.downloads();
        let updated = fx.finish_windows();
        assert!(updated.recovered);
        // Finished from the cached installers.
        assert_eq!(fx.downloads(), downloads);
        assert_installed(&fx);
        assert_eq!(fx.read("saves/mine.sav"), "mine");
    }
}

#[test]
fn a_dlc_that_never_started_is_failed_and_one_partway_blocks_the_game() {
    let fx = offered();
    fx.exits(DLC, 1);
    let installed = fx.install_windows(&request()).unwrap();
    assert_eq!(
        fx.statuses_windows(),
        [
            (RIMWORLD, ProductStatus::Installed),
            (DLC, ProductStatus::Failed)
        ]
    );
    assert_eq!(
        installed.warnings,
        [format!(
            "Royalty ({DLC}) failed and wasn't installed: its installer stopped before installing anything (exit code 1)."
        )]
    );
    assert!(!fx.launch_refused());

    let fx = offered();
    fx.exits(DLC, 5);
    let error = fx.install_windows(&request()).unwrap_err();
    assert!(
        matches!(error.downcast_ref(), Some(SetupError::Partway { product, .. }) if *product == DLC),
        "{error:#}"
    );
    assert!(fx.launch_refused());
    fs::remove_file(fx.runs().join(format!("exit-{DLC}"))).unwrap();
    fx.finish_windows();
    assert_installed(&fx);
    // Finishing ran every installer again, the game's first.
    assert_eq!(
        fx.ran(),
        [
            format!("setup_{RIMWORLD}_1_0.exe"),
            format!("setup_{DLC}_1_0.exe"),
            format!("setup_{RIMWORLD}_1_0.exe"),
            format!("setup_{DLC}_1_0.exe"),
        ]
    );
}

#[test]
fn a_dlc_with_no_windows_installer_is_unavailable() {
    let fx = offered();
    fx.gog.lock().unwrap().windows.remove(&DLC);
    let plan = fx.plan_windows(&request()).unwrap();
    assert_eq!(plan.products()[1].status, ProductStatus::Unavailable);
    assert_eq!(
        plan.products()[1].note.as_deref(),
        Some(format!("Royalty ({DLC}) has no Windows installer.").as_str())
    );
    install::apply(&fx.paths, &mut fx.session(), &plan, &Token::new(), |_| {}).unwrap();
    assert_eq!(
        fx.statuses_windows(),
        [
            (RIMWORLD, ProductStatus::Installed),
            (DLC, ProductStatus::Unavailable)
        ]
    );
    assert!(!fx.launch_refused());
}

#[test]
fn a_canceled_installer_keeps_the_journal() {
    let fx = offered();
    let plan = fx.plan_windows(&request()).unwrap();
    let token = Token::new();
    let error = install::apply_with_events(
        &fx.paths,
        &mut fx.session(),
        &plan,
        &token,
        |_| {},
        |event| {
            if let Event::Installing { product: DLC, .. } = event {
                token.cancel();
            }
        },
    )
    .unwrap_err();
    assert!(
        matches!(
            error.downcast_ref(),
            Some(UpdateError::Canceled { journal: true })
        ),
        "{error:#}"
    );
    assert_eq!(fx.state().journal.unwrap().kind, JournalKind::Install);
    assert_eq!(fx.read("Data/core.dat"), "core v1");
    fx.finish_windows();
    assert_installed(&fx);
}

#[test]
fn an_install_interrupted_at_each_step_is_finished_by_update_or_installing_again() {
    for (at, by_update) in [
        ("install.create.before", false),
        ("install.state.after", false),
        ("install.windows.base.after", true),
        ("install.windows.base.after", false),
        ("install.windows.dlc.after", true),
        ("install.windows.dlc.after", false),
        ("install.windows.commit.before", true),
        ("install.windows.commit.before", false),
    ] {
        let fx = offered();
        set_failpoint(Some(at));
        let error = fx.install_windows(&request()).unwrap_err();
        assert!(format!("{error:#}").contains("injected"), "{at}: {error:#}");
        if at == "install.create.before" {
            assert!(!fx.folder.exists());
        } else {
            assert!(fx.launch_refused(), "{at}");
        }
        let downloads = fx.downloads();
        if by_update {
            fx.finish_windows();
        } else {
            let plan = fx.plan_windows(&request()).unwrap();
            assert_eq!(plan.resumes(), at != "install.create.before", "{at}");
            install::apply(&fx.paths, &mut fx.session(), &plan, &Token::new(), |_| {}).unwrap();
        }
        assert_eq!(fx.downloads(), downloads, "{at}");
        assert_installed(&fx);
    }
}

#[test]
fn an_install_resumed_without_its_cached_installers_downloads_them_again() {
    let fx = offered();
    fx.exits(RIMWORLD, 5);
    fx.install_windows(&request()).unwrap_err();
    fs::remove_file(fx.runs().join(format!("exit-{RIMWORLD}"))).unwrap();
    fs::remove_dir_all(fx.paths.gog_download_cache()).unwrap();
    let downloads = fx.downloads();
    fx.finish_windows();
    assert_eq!(fx.downloads(), downloads + 4);
    assert_installed(&fx);
}

#[test]
fn the_environment_and_prefix_stay_reserved_between_installers() {
    let fx = offered();
    let plan = fx.plan_windows(&request()).unwrap();
    let prefix = GlobalConfig::default()
        .prefix_root(&fx.paths)
        .unwrap()
        .join("rimworld");
    let mut checked = Vec::new();
    install::apply_with_events(
        &fx.paths,
        &mut fx.session(),
        &plan,
        &Token::new(),
        |_| {},
        |event| {
            if let Event::Installing { product, .. } = event {
                // A snapshot restore, which would replace the prefix, is
                // refused before creation and between installers.
                let refused = Locks::new(&fx.paths)
                    .acquire(Operation::Snapshot, "rimworld", &[&prefix])
                    .is_err();
                checked.push((product, refused));
            }
        },
    )
    .unwrap();
    assert_eq!(checked, [(RIMWORLD, true), (DLC, true)]);
    assert_installed(&fx);
    // Let go once committed.
    assert!(
        Locks::new(&fx.paths)
            .acquire(Operation::Snapshot, "rimworld", &[&prefix])
            .is_ok()
    );
}

#[test]
fn finishing_with_update_also_keeps_the_prefix_reserved() {
    let fx = offered();
    fx.exits(DLC, 5);
    fx.install_windows(&request()).unwrap_err();
    fs::remove_file(fx.runs().join(format!("exit-{DLC}"))).unwrap();
    let prefix = GlobalConfig::default()
        .prefix_root(&fx.paths)
        .unwrap()
        .join("rimworld");
    let paths = fx.paths.clone();
    let refused = std::rc::Rc::new(Cell::new(false));
    let seen = std::rc::Rc::clone(&refused);
    set_before_target(move || {
        seen.set(
            Locks::new(&paths)
                .acquire(Operation::Snapshot, "rimworld", &[&prefix])
                .is_err(),
        );
    });
    fx.finish_windows();
    assert!(refused.get());
    assert_installed(&fx);
}

#[test]
fn a_cancel_after_the_installers_stops_before_the_target_and_commit() {
    let fx = offered();
    let plan = fx.plan_windows(&request()).unwrap();
    let token = Token::new();
    let canceler = token.clone();
    set_before_target(move || canceler.cancel());
    let error = install::apply(&fx.paths, &mut fx.session(), &plan, &token, |_| {}).unwrap_err();
    assert!(
        matches!(
            error.downcast_ref(),
            Some(UpdateError::Canceled { journal: true })
        ),
        "{error:#}"
    );
    assert_eq!(fx.state().journal.unwrap().kind, JournalKind::Install);
    let environment = Environment::load(&fx.paths, "rimworld").unwrap();
    assert!(environment.config.targets.is_empty());
    fx.finish_windows();
    assert_installed(&fx);
}

#[test]
fn an_info_file_that_isnt_a_small_regular_file_isnt_read() {
    for make in [
        format!("mkfifo \"$DIR/goggame-{RIMWORLD}.info\""),
        format!("head -c 200000 /dev/zero > \"$DIR/goggame-{RIMWORLD}.info\""),
    ] {
        let fx = Fixture::windows();
        fx.offer_windows(
            RIMWORLD,
            "RimWorld",
            "1.0",
            &[("bin/Game.exe", "game")],
            None,
        );
        // Written by the setup program in place of GOG's info file.
        {
            let mut gog = fx.gog.lock().unwrap();
            let setup = &mut gog.windows.get_mut(&RIMWORLD).unwrap().files[0].1;
            let text = String::from_utf8(setup.clone()).unwrap();
            *setup = text
                .replace("exit \"$code\"\n", &format!("{make}\nexit \"$code\"\n"))
                .into_bytes();
        }
        fx.owns_ids(&[RIMWORLD]);
        let installed = fx.install_windows(&request()).unwrap();
        assert_eq!(installed.target, None);
        assert_eq!(installed.reported_language, None);
        assert_eq!(fx.state().journal, None);
    }
}

// Updates and reinstalls of an installed Windows game.

/// RimWorld and its DLC installed from their 1.0 installers, and a save of
/// yours in the folder.
fn installed_windows() -> Fixture {
    let fx = offered();
    fx.install_windows(&request()).unwrap();
    assert_installed(&fx);
    fx.write("saves/mine.sav", "mine");
    fx
}

/// GOG offers 2.0 of the game.
fn base_v2(fx: &Fixture) {
    fx.offer_windows(
        RIMWORLD,
        "RimWorld",
        "2.0",
        &[("bin/Game.exe", "game"), ("Data/core.dat", "core v2")],
        Some(english(RIMWORLD)),
    );
}

/// GOG offers 2.0 of the DLC.
fn dlc_v2(fx: &Fixture) {
    fx.offer_windows(
        DLC,
        "RimWorld - Royalty",
        "2.0",
        &[("Data/Royalty/r.dat", "royalty v2")],
        Some(info(DLC, &["en-US"], "English", serde_json::json!([]))),
    );
}

fn setup(id: u64, version: &str) -> String {
    format!("setup_{id}_{}.exe", version.replace('.', "_"))
}

/// The installers each update ran, after the install's.
fn ran_since_install(fx: &Fixture) -> Vec<String> {
    fx.ran()[2..].to_vec()
}

fn versions(fx: &Fixture) -> Vec<(u64, Option<String>)> {
    fx.state()
        .committed
        .unwrap()
        .products
        .iter()
        .map(|product| (product.id, product.version.clone()))
        .collect()
}

fn v(version: &str) -> Option<String> {
    Some(version.to_owned())
}

/// Committed, launchable, with nothing in flight and your files there.
fn assert_settled(fx: &Fixture) {
    let state = fx.state();
    assert_eq!(state.journal, None);
    assert!(!fx.launch_refused());
    assert_eq!(fx.read("saves/mine.sav"), "mine");
    assert_eq!(cached(fx, RIMWORLD), 0);
    assert_eq!(cached(fx, DLC), 0);
}

fn prefix(fx: &Fixture) -> PathBuf {
    GlobalConfig::default()
        .prefix_root(&fx.paths)
        .unwrap()
        .join("rimworld")
}

#[test]
fn a_check_finds_a_windows_update_and_the_update_runs_the_newer_installer() {
    let fx = installed_windows();
    base_v2(&fx);
    let status = crate::gog::update::check(&mut fx.session(), "rimworld", true).unwrap();
    assert_eq!(status.state, UpdateState::Available);
    assert!(status.applicable);
    assert_eq!(status.reason, None);
    assert_eq!(
        (status.installed.as_deref(), status.available.as_deref()),
        (Some("1.0"), Some("2.0"))
    );

    let pending = fx.pending(true, Options::default());
    assert_eq!(pending.kind(), JournalKind::Update);
    assert!(!pending.is_recovery());
    // Only the game's installer runs; the DLC has nothing newer.
    let planned: Vec<_> = pending
        .products()
        .iter()
        .map(|product| (product.id, product.version.clone()))
        .collect();
    assert_eq!(planned, [(RIMWORLD, v("2.0")), (DLC, None)]);
    assert_eq!(pending.unpacked_estimate(), 2 * pending.download_size());

    let updated = fx
        .apply_with(true, &pending, &Token::new(), |_| {})
        .unwrap();
    assert_eq!(updated.kind, JournalKind::Update);
    assert!(!updated.recovered);
    assert!(updated.warnings.is_empty(), "{:?}", updated.warnings);
    assert_eq!(ran_since_install(&fx), [setup(RIMWORLD, "2.0")]);
    assert_eq!(versions(&fx), [(RIMWORLD, v("2.0")), (DLC, v("1.0"))]);
    assert_eq!(fx.read("Data/core.dat"), "core v2");
    assert_eq!(fx.read("Data/Royalty/r.dat"), "royalty v1");
    assert_settled(&fx);

    // The same silent Inno arguments as installing.
    let args = fx.args(RIMWORLD);
    assert!(args[0].ends_with(&setup(RIMWORLD, "2.0")), "{args:?}");
    assert_eq!(
        args[1..6],
        [
            "/SP-",
            "/SILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/NOCANCEL"
        ]
    );
    let dir = format!(
        "/DIR=Z:{}",
        fx.folder.display().to_string().replace('/', "\\")
    );
    assert!(args.contains(&dir), "{args:?}");

    // The default target stays as it was.
    let environment = Environment::load(&fx.paths, "rimworld").unwrap();
    assert_eq!(environment.config.targets.len(), 1);
    assert!(matches!(
        fx.plan_with(true, Options::default()).unwrap(),
        Plan::UpToDate { .. }
    ));
}

#[test]
fn each_installed_dlc_with_a_newer_version_runs_after_the_game() {
    let fx = installed_windows();
    base_v2(&fx);
    dlc_v2(&fx);
    fx.update();
    assert_eq!(
        ran_since_install(&fx),
        [setup(RIMWORLD, "2.0"), setup(DLC, "2.0")]
    );
    assert_eq!(versions(&fx), [(RIMWORLD, v("2.0")), (DLC, v("2.0"))]);
    assert_eq!(fx.read("Data/Royalty/r.dat"), "royalty v2");
    assert_settled(&fx);
}

#[test]
fn a_dlc_only_update_runs_only_the_dlc() {
    let fx = installed_windows();
    dlc_v2(&fx);
    let status = crate::gog::update::check(&mut fx.session(), "rimworld", true).unwrap();
    assert!(status.applicable);
    fx.update();
    assert_eq!(ran_since_install(&fx), [setup(DLC, "2.0")]);
    assert_eq!(versions(&fx), [(RIMWORLD, v("1.0")), (DLC, v("2.0"))]);
    assert_settled(&fx);
}

#[test]
fn a_patch_gog_lists_is_never_used_and_the_full_installer_runs() {
    let fx = installed_windows();
    base_v2(&fx);
    // Even one whose name says it starts from the installed version: GOG
    // names no starting version wineo can trust.
    fx.gog.lock().unwrap().patches.insert(
        RIMWORLD,
        vec![
            ("Patch (1.0 to 2.0)".to_owned(), "2.0".to_owned()),
            ("RimWorld (32Bit Installer)".to_owned(), "2.0".to_owned()),
        ],
    );
    let downloads = fx.downloads();
    fx.update();
    // The setup program and its part.
    assert_eq!(fx.downloads(), downloads + 2);
    assert_eq!(ran_since_install(&fx), [setup(RIMWORLD, "2.0")]);
    assert_eq!(versions(&fx)[0], (RIMWORLD, v("2.0")));
    assert_settled(&fx);
}

#[test]
fn a_reinstall_runs_every_current_installer_again() {
    let fx = installed_windows();
    fs::remove_file(fx.folder.join("Data/core.dat")).unwrap();
    let updated = fx.reinstall();
    assert_eq!(updated.kind, JournalKind::Reinstall);
    assert_eq!(
        ran_since_install(&fx),
        [setup(RIMWORLD, "1.0"), setup(DLC, "1.0")]
    );
    assert_eq!(fx.read("Data/core.dat"), "core v1");
    assert_eq!(versions(&fx), [(RIMWORLD, v("1.0")), (DLC, v("1.0"))]);
    assert_settled(&fx);

    // With newer installers on GOG, the current ones.
    base_v2(&fx);
    fx.reinstall();
    assert_eq!(
        ran_since_install(&fx)[2..],
        [setup(RIMWORLD, "2.0"), setup(DLC, "1.0")]
    );
    assert_eq!(versions(&fx), [(RIMWORLD, v("2.0")), (DLC, v("1.0"))]);
    assert_settled(&fx);
}

#[test]
fn without_a_graphical_session_an_update_is_refused_before_anything() {
    let fx = installed_windows();
    base_v2(&fx);
    set_graphical(false);
    for reinstall in [false, true] {
        let error = fx
            .plan_with(
                true,
                Options {
                    reinstall,
                    ..Options::default()
                },
            )
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref(),
            Some(SetupError::NoGraphicalSession)
        ));
    }
    set_graphical(true);
    assert_eq!(versions(&fx)[0], (RIMWORLD, v("1.0")));
}

#[test]
fn an_update_interrupted_at_each_step_is_finished_by_update() {
    for at in [
        "update.journal.before",
        "update.journal.after",
        "install.windows.base.after",
        "install.windows.dlc.after",
        "install.windows.commit.before",
    ] {
        let fx = installed_windows();
        base_v2(&fx);
        dlc_v2(&fx);
        set_failpoint(Some(at));
        fx.update_err();
        let downloads = fx.downloads();
        if at == "update.journal.before" {
            // Nothing changed, so it's an update like any other.
            assert_eq!(fx.state().journal, None, "{at}");
            assert!(!fx.launch_refused(), "{at}");
            fx.update();
        } else {
            let journal = fx.state().journal.unwrap();
            assert_eq!(journal.kind, JournalKind::Update, "{at}");
            assert_eq!(journal.attempted[0].path, ".", "{at}");
            assert!(fx.launch_refused(), "{at}");
            assert_eq!(fx.read("saves/mine.sav"), "mine");
            // Finished from the installers already downloaded, without GOG.
            let pending = fx.pending(false, Options::default());
            assert!(pending.is_recovery() && pending.is_offline(), "{at}");
            assert_eq!(pending.kind(), JournalKind::Update, "{at}");
            let updated = fx
                .apply_with(false, &pending, &Token::new(), |_| {})
                .unwrap();
            assert!(updated.recovered, "{at}");
        }
        assert_eq!(fx.downloads(), downloads, "{at}");
        assert_eq!(
            versions(&fx),
            [(RIMWORLD, v("2.0")), (DLC, v("2.0"))],
            "{at}"
        );
        assert_eq!(fx.read("Data/core.dat"), "core v2", "{at}");
        assert_settled(&fx);
    }
}

#[test]
fn a_reinstall_interrupted_is_finished_as_a_reinstall() {
    let fx = installed_windows();
    set_failpoint(Some("install.windows.base.after"));
    let error = fx
        .try_update(Options {
            reinstall: true,
            ..Options::default()
        })
        .unwrap_err();
    assert!(format!("{error:#}").contains("injected"), "{error:#}");
    assert_eq!(fx.state().journal.unwrap().kind, JournalKind::Reinstall);
    let pending = fx.pending(true, Options::default());
    assert_eq!(pending.kind(), JournalKind::Reinstall);
    fx.apply_with(true, &pending, &Token::new(), |_| {})
        .unwrap();
    assert_eq!(
        ran_since_install(&fx),
        [
            setup(RIMWORLD, "1.0"),
            setup(RIMWORLD, "1.0"),
            setup(DLC, "1.0")
        ]
    );
    assert_settled(&fx);
}

#[test]
fn a_dlc_only_update_interrupted_reruns_only_the_dlc() {
    let fx = installed_windows();
    dlc_v2(&fx);
    fx.exits(DLC, 5);
    fx.try_update(Options::default()).unwrap_err();
    assert_eq!(fx.state().journal.unwrap().packages.len(), 2);
    fs::remove_file(fx.runs().join(format!("exit-{DLC}"))).unwrap();

    // With the cached installer gone, GOG's current one, for the same
    // product.
    fx.clear_cache();
    let downloads = fx.downloads();
    let pending = fx.pending(true, Options::default());
    assert!(pending.is_recovery() && !pending.is_offline());
    fx.apply_with(true, &pending, &Token::new(), |_| {})
        .unwrap();
    assert_eq!(fx.downloads(), downloads + 2);
    assert_eq!(
        ran_since_install(&fx),
        [setup(DLC, "2.0"), setup(DLC, "2.0")]
    );
    assert_eq!(versions(&fx), [(RIMWORLD, v("1.0")), (DLC, v("2.0"))]);
    assert_settled(&fx);
}

#[test]
fn a_failed_update_keeps_every_file_until_update_finishes_it() {
    for code in [4, 5, 6, 8, 9] {
        let fx = installed_windows();
        base_v2(&fx);
        fx.exits(RIMWORLD, code);
        let error = fx.try_update(Options::default()).unwrap_err();
        assert!(
            matches!(error.downcast_ref(), Some(SetupError::Partway { product, .. }) if *product == RIMWORLD),
            "{error:#}"
        );
        let journal = fx.state().journal.unwrap();
        assert_eq!(journal.kind, JournalKind::Update);
        assert!(fx.launch_refused());
        // What the installer wrote stays, and so does everything else.
        assert_eq!(fx.read("Data/core.dat"), "core v2");
        assert_eq!(fx.read("Data/Royalty/r.dat"), "royalty v1");
        assert_eq!(fx.read("saves/mine.sav"), "mine");
        assert_eq!(versions(&fx)[0], (RIMWORLD, v("1.0")));

        fs::remove_file(fx.runs().join(format!("exit-{RIMWORLD}"))).unwrap();
        let pending = fx.pending(true, Options::default());
        assert!(pending.is_recovery());
        fx.apply_with(true, &pending, &Token::new(), |_| {})
            .unwrap();
        assert_eq!(versions(&fx), [(RIMWORLD, v("2.0")), (DLC, v("1.0"))]);
        assert_settled(&fx);
    }
}

#[test]
fn an_update_whose_installer_never_started_offers_its_wizard_once() {
    for code in [1, 3, 7] {
        let fx = installed_windows();
        base_v2(&fx);
        fx.exits(RIMWORLD, code);
        let before = fx.state();
        let error = fx.try_update(Options::default()).unwrap_err();
        match error.downcast_ref() {
            Some(SetupError::NotStarted {
                product,
                code: found,
                wizard,
                ..
            }) => assert_eq!((*product, *found, *wizard), (RIMWORLD, code, true)),
            _ => panic!("{error:#}"),
        }
        // Nothing changed, so the game is as it was and still launches,
        // with the update still to do.
        assert_eq!(fx.state(), before);
        assert!(!fx.launch_refused());
        let status = crate::gog::update::check(&mut fx.session(), "rimworld", true).unwrap();
        assert!(status.applicable);

        // The installer's own wizard, without the flags that hide it.
        let wizard = Options {
            wizard: true,
            ..Options::default()
        };
        let pending = fx.pending(true, wizard);
        assert!(pending.wizard());
        let error = fx
            .apply_with(true, &pending, &Token::new(), |_| {})
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref(),
            Some(SetupError::NotStarted { wizard: false, .. })
        ));
        let args = fx.args(RIMWORLD);
        assert!(!args.contains(&"/SILENT".to_owned()), "{args:?}");
        assert_eq!(fx.state(), before);

        fs::remove_file(fx.runs().join(format!("exit-{RIMWORLD}"))).unwrap();
        fx.try_update(wizard).unwrap();
        assert_eq!(versions(&fx)[0], (RIMWORLD, v("2.0")));
        assert_settled(&fx);
    }
}

#[test]
fn an_update_canceled_by_its_installer_before_changing_anything_leaves_the_game() {
    let fx = installed_windows();
    base_v2(&fx);
    fx.exits(RIMWORLD, 2);
    let before = fx.state();
    let error = fx.try_update(Options::default()).unwrap_err();
    assert!(
        matches!(error.downcast_ref(), Some(SetupError::Canceled { product, .. }) if *product == RIMWORLD),
        "{error:#}"
    );
    assert_eq!(fx.state(), before);
    assert!(!fx.launch_refused());
}

#[test]
fn finishing_an_update_whose_installer_never_started_keeps_the_journal() {
    let fx = installed_windows();
    base_v2(&fx);
    fx.exits(RIMWORLD, 5);
    fx.try_update(Options::default()).unwrap_err();
    // An earlier attempt may have changed files, so this one stopping
    // early doesn't make the game whole.
    fx.exits(RIMWORLD, 1);
    let error = fx.try_update(Options::default()).unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(SetupError::NotStarted { wizard: true, .. })
    ));
    assert_eq!(fx.state().journal.unwrap().kind, JournalKind::Update);
    assert!(fx.launch_refused());
}

#[test]
fn a_dlc_whose_newer_installer_never_started_stays_as_it_was() {
    let fx = installed_windows();
    base_v2(&fx);
    dlc_v2(&fx);
    fx.exits(DLC, 1);
    let updated = fx.update();
    assert_eq!(
        updated.warnings,
        [format!(
            "Royalty ({DLC}) wasn't updated: its installer stopped before installing anything (exit code 1)."
        )]
    );
    assert_eq!(versions(&fx), [(RIMWORLD, v("2.0")), (DLC, v("1.0"))]);
    assert_eq!(
        fx.statuses_windows(),
        [
            (RIMWORLD, ProductStatus::Installed),
            (DLC, ProductStatus::Installed)
        ]
    );
    assert_settled(&fx);
    // So the DLC's update is offered again.
    let status = crate::gog::update::check(&mut fx.session(), "rimworld", true).unwrap();
    assert!(status.applicable);
}

#[test]
fn canceling_an_update_stops_at_its_checkpoints() {
    // While checking the downloaded installers, before anything changed.
    let fx = installed_windows();
    base_v2(&fx);
    let before = fx.state();
    let pending = fx.pending(true, Options::default());
    let token = Token::new();
    let error = fx
        .apply_with(true, &pending, &token, |step| {
            if matches!(step, Progress::Verify { .. }) {
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
    assert_eq!(fx.state(), before);
    assert_eq!(fx.ran().len(), 2);

    // After the installers, before the commit: the journal stays.
    let token = Token::new();
    let canceler = token.clone();
    set_before_target(move || canceler.cancel());
    let pending = fx.pending(true, Options::default());
    let error = fx.apply_with(true, &pending, &token, |_| {}).unwrap_err();
    assert!(
        matches!(
            error.downcast_ref(),
            Some(UpdateError::Canceled { journal: true })
        ),
        "{error:#}"
    );
    assert_eq!(fx.state().journal.unwrap().kind, JournalKind::Update);
    assert!(fx.launch_refused());
    let pending = fx.pending(false, Options::default());
    fx.apply_with(false, &pending, &Token::new(), |_| {})
        .unwrap();
    assert_eq!(versions(&fx)[0], (RIMWORLD, v("2.0")));
    assert_settled(&fx);
}

#[test]
fn an_update_keeps_the_prefix_reserved_to_its_commit() {
    let fx = installed_windows();
    base_v2(&fx);
    let (paths, held) = (fx.paths.clone(), prefix(&fx));
    let refused = std::rc::Rc::new(Cell::new(false));
    let seen = std::rc::Rc::clone(&refused);
    set_before_target(move || {
        // A snapshot restore would replace the prefix mid-update.
        seen.set(
            Locks::new(&paths)
                .acquire(Operation::Snapshot, "rimworld", &[&held])
                .is_err(),
        );
    });
    fx.update();
    assert!(refused.get());
    assert!(
        Locks::new(&fx.paths)
            .acquire(Operation::Snapshot, "rimworld", &[&prefix(&fx)])
            .is_ok()
    );
}
