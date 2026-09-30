//! Registering a planned Heroic import.
//!
//! Applying takes two calls, `apply` and then `Registered::finish`, because a
//! frontend fetches art between them: the desktop entry is written last so it
//! gets the art's icon from the start.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::art::{self, Kind};
use crate::cancel::Token;
use crate::config::GlobalConfig;
use crate::desktop::{self, Change};
use crate::environment::Environment;
use crate::heroic::HeroicSource;
use crate::import::{self, ImportProgress};
use crate::paths::Paths;
use crate::runner::Runner;
use crate::stats;

use super::{GamePlan, NativePlan};

/// Why an apply was refused before it wrote anything.
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("Heroic settings changed while they were being reviewed; inspect the game again")]
    SourceChanged,
}

/// What became of the playtime Heroic tracked for the game. Never fails an
/// import: a game worth playing is worth importing even if its history can't be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Playtime {
    /// Heroic never played it, so there was nothing to carry over.
    None,
    Imported,
    /// Heroic's playtime couldn't be read. Holds why.
    Unreadable(String),
    /// The environment's play log couldn't be written. Holds why.
    NotRecorded(String),
}

/// An environment that's registered, with its desktop entries still to write.
#[derive(Debug)]
pub struct Registered {
    pub environment: Environment,
    pub playtime: Playtime,
    runner: Option<Runner>,
    destination: Option<PathBuf>,
}

impl Registered {
    /// The runner the environment was imported with. None for a native game.
    pub fn runner(&self) -> Option<&Runner> {
        self.runner.as_ref()
    }

    /// Where the prefix lives. None for a native game.
    pub fn destination(&self) -> Option<&Path> {
        self.destination.as_deref()
    }

    /// Brings the environment's desktop entries in line with its config, and
    /// says what changed.
    pub fn finish(&self, paths: &Paths, global: &GlobalConfig) -> crate::Result<Vec<Change>> {
        match (&self.runner, &self.destination) {
            (Some(runner), Some(destination)) => {
                desktop::reconcile(paths, global, &self.environment, Some(runner), destination)
            }
            _ => desktop::reconcile_native(paths, global, &self.environment),
        }
    }
}

/// Registers `plan` as an environment, copying or adopting its prefix, and
/// carries over Heroic's playtime. Refuses when Heroic's settings changed since
/// the game was inspected.
pub fn apply(
    paths: &Paths,
    global: &GlobalConfig,
    source: &HeroicSource,
    plan: &GamePlan,
    token: &Token,
    progress: impl FnMut(ImportProgress),
) -> anyhow::Result<Registered> {
    ensure_unchanged(source, &plan.heroic().id, &plan.heroic().fingerprint)?;
    let settings = plan.settings();
    let environment = import::execute_reviewed(
        paths,
        global,
        plan.request().clone(),
        Some(plan.inspection()),
        Some(&settings),
        token,
        progress,
    )?;
    let playtime = import_playtime(paths, source, &plan.heroic().id, &environment.name);
    Ok(Registered {
        environment,
        playtime,
        runner: Some(plan.runner().clone()),
        destination: Some(plan.destination().to_owned()),
    })
}

/// Registers `plan` as a native environment, which has no prefix, and carries
/// over Heroic's playtime. Refuses when Heroic's settings changed since the game
/// was inspected.
pub fn apply_native(
    paths: &Paths,
    source: &HeroicSource,
    plan: &NativePlan,
) -> anyhow::Result<Registered> {
    ensure_unchanged(source, &plan.heroic().id, &plan.heroic().fingerprint)?;
    let environment = import::register_native(paths, plan.name(), &plan.settings())?;
    let playtime = import_playtime(paths, source, &plan.heroic().id, &environment.name);
    Ok(Registered {
        environment,
        playtime,
        runner: None,
        destination: None,
    })
}

/// Stores Heroic's cached `poster` as `env`'s Grid art, recorded as a manual
/// pick named `heroic-<app_name>.<ext>` so its origin is visible. Returns None
/// without storing anything when the file is gone, too large, or not a format
/// Grid art can be. Heroic's file is only read.
pub fn store_poster(
    paths: &Paths,
    env: &str,
    app_name: &str,
    poster: &Path,
) -> crate::Result<Option<art::Stored>> {
    let mut bytes = Vec::new();
    let read = fs::File::open(poster)
        .and_then(|file| file.take(art::MAX_BYTES + 1).read_to_end(&mut bytes));
    match read {
        Ok(_) => {}
        // Heroic may have pruned its cache since the game was listed.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(crate::error::Error::Io {
                context: format!("failed to read {}", poster.display()),
                source: error,
            });
        }
    }
    if bytes.len() as u64 > art::MAX_BYTES {
        return Ok(None);
    }
    let Some(format) = art::detect(&bytes).filter(|format| Kind::Grid.accepts(*format)) else {
        return Ok(None);
    };
    art::set_manual_bytes(
        paths,
        env,
        None,
        Kind::Grid,
        &bytes,
        &poster_name(app_name, format),
    )
    .map(Some)
}

/// The source name a Heroic poster is recorded under, so its provenance reads
/// as a manual pick from Heroic.
pub fn poster_name(app_name: &str, format: art::Format) -> String {
    format!("heroic-{app_name}.{}", format.extension())
}

pub(super) fn ensure_unchanged(
    source: &HeroicSource,
    id: &str,
    fingerprint: &crate::heroic::InputFingerprint,
) -> anyhow::Result<()> {
    if source.unchanged(id, fingerprint)? {
        Ok(())
    } else {
        Err(ApplyError::SourceChanged.into())
    }
}

/// Carries `game_id`'s Heroic playtime into `name`'s new play log, if Heroic
/// ever played it.
pub(super) fn import_playtime(
    paths: &Paths,
    source: &HeroicSource,
    game_id: &str,
    name: &str,
) -> Playtime {
    match source.playtime(game_id) {
        Ok(Some(playtime)) => {
            let imported = stats::Imported {
                seconds: playtime.seconds,
                last_played: playtime.last_played,
            };
            match stats::set_imported(paths, name, imported) {
                Ok(()) => Playtime::Imported,
                Err(error) => Playtime::NotRecorded(format!("{:#}", anyhow::Error::from(error))),
            }
        }
        Ok(None) => Playtime::None,
        Err(error) => Playtime::Unreadable(format!("{:#}", anyhow::Error::from(error))),
    }
}

/// Appends `body` to `name`'s import log as a timestamped section, and says
/// where the log is.
pub fn write_import_log(paths: &Paths, name: &str, body: &str) -> anyhow::Result<PathBuf> {
    let path = paths.import_log(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    writeln!(file, "=== {} ===", jiff::Timestamp::now())?;
    write!(file, "{body}")?;
    if !body.ends_with('\n') {
        writeln!(file)?;
    }
    writeln!(file)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::config::Defaults;
    use crate::heroic_import::plan::tests::{fake_wine_runner, paths};
    use crate::heroic_import::{Options, plan_game};

    /// A Heroic config with one sideloaded game, Alpha, whose prefix is
    /// `root/heroic-prefix`, and a fake wine runner named `fake`.
    fn library(root: &Path) -> (Paths, GlobalConfig, HeroicSource) {
        let prefix = root.join("heroic-prefix");
        fs::create_dir_all(prefix.join("drive_c/Games")).unwrap();
        fs::write(
            prefix.join("system.reg"),
            "WINE REGISTRY Version 2\n#arch=win64\n",
        )
        .unwrap();
        let exe = prefix.join("drive_c/Games/Game.exe");
        fs::write(&exe, "game executable").unwrap();
        let config = root.join("heroic");
        fs::create_dir_all(config.join("sideload_apps")).unwrap();
        fs::create_dir_all(config.join("GamesConfig")).unwrap();
        fs::create_dir_all(config.join("store")).unwrap();
        fs::write(
            config.join("config.json"),
            format!(
                r#"{{"defaultSettings":{{"winePrefix":"{}","wineVersion":{{"type":"wine","name":"System","bin":"wine"}}}}}}"#,
                prefix.display()
            ),
        )
        .unwrap();
        fs::write(config.join("GamesConfig/a.json"), r#"{"a":{}}"#).unwrap();
        fs::write(
            config.join("sideload_apps/library.json"),
            format!(
                r#"{{"games":[{{"app_name":"a","title":"Alpha","runner":"sideload","install":{{"platform":"Windows","executable":"{}"}}}}]}}"#,
                exe.display()
            ),
        )
        .unwrap();
        let runner_root = root.join("runners");
        fake_wine_runner(&runner_root, "fake");
        let global = GlobalConfig {
            defaults: Defaults {
                runner_root: Some(runner_root.display().to_string()),
                prefix_root: Some(root.join("prefixes").display().to_string()),
                ..Defaults::default()
            },
            ..GlobalConfig::default()
        };
        let source = HeroicSource::from_config_dir_with_home(config, root.to_owned());
        (paths(root), global, source)
    }

    fn plan(paths: &Paths, global: &GlobalConfig, source: &HeroicSource) -> GamePlan {
        plan_game(paths, global, source, "a", "fake", Options::default()).unwrap()
    }

    #[test]
    fn apply_registers_the_plan_and_carries_playtime() {
        let root = tempdir().unwrap();
        let (paths, global, source) = library(root.path());
        fs::write(
            source.config_dir().join("store/timestamp.json"),
            r#"{"a":{"totalPlayed":90,"lastPlayed":"2024-01-02T03:04:05Z"}}"#,
        )
        .unwrap();
        let mut plan = plan(&paths, &global, &source);
        plan.set_desktop_enabled(true);
        let mut events = Vec::new();
        let registered = apply(&paths, &global, &source, &plan, &Token::new(), |event| {
            events.push(event)
        })
        .unwrap();
        assert_eq!(registered.environment.name, "alpha");
        assert_eq!(
            registered.environment.config.heroic_id.as_deref(),
            Some("a")
        );
        assert_eq!(registered.playtime, Playtime::Imported);
        assert_eq!(registered.runner().unwrap().name(), "fake");
        let copy = root.path().join("prefixes/alpha");
        assert_eq!(registered.destination(), Some(copy.as_path()));
        assert!(copy.join("system.reg").is_file());
        assert!(matches!(
            events.first(),
            Some(ImportProgress::Started { .. })
        ));
        assert!(matches!(
            events.last(),
            Some(ImportProgress::Finished { .. })
        ));
        let log = fs::read_to_string(paths.stats_log("alpha")).unwrap();
        assert!(log.contains("5400"), "{log}");

        // The desktop entry is written by the second call.
        let changes = registered.finish(&paths, &global).unwrap();
        assert!(
            changes
                .iter()
                .any(|change| matches!(change, Change::Wrote { .. })),
            "{changes:?}"
        );
    }

    #[test]
    fn apply_can_adopt_and_leaves_the_prefix_where_it_is() {
        let root = tempdir().unwrap();
        let (paths, global, source) = library(root.path());
        let plan = plan_game(
            &paths,
            &global,
            &source,
            "a",
            "fake",
            Options {
                name: Some("mine".to_owned()),
                adopt: true,
                ..Options::default()
            },
        )
        .unwrap();
        let registered = apply(&paths, &global, &source, &plan, &Token::new(), |_| {}).unwrap();
        assert_eq!(registered.playtime, Playtime::None);
        assert_eq!(
            registered.destination(),
            Some(root.path().join("heroic-prefix").as_path())
        );
        assert!(!root.path().join("prefixes").exists());
        assert!(registered.finish(&paths, &global).unwrap().is_empty());
    }

    #[test]
    fn apply_refuses_when_heroic_changed_since_the_game_was_inspected() {
        let root = tempdir().unwrap();
        let (paths, global, source) = library(root.path());
        let plan = plan(&paths, &global, &source);
        fs::write(
            source.config_dir().join("GamesConfig/a.json"),
            r#"{"a":{"useGameMode":true}}"#,
        )
        .unwrap();
        let error = apply(&paths, &global, &source, &plan, &Token::new(), |_| {}).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ApplyError>(),
            Some(ApplyError::SourceChanged)
        ));
        assert!(!paths.environment_config("alpha").exists());
    }

    #[test]
    fn a_canceled_apply_leaves_nothing_behind() {
        let root = tempdir().unwrap();
        let (paths, global, source) = library(root.path());
        let plan = plan(&paths, &global, &source);
        let token = Token::new();
        token.cancel();
        let error = apply(&paths, &global, &source, &plan, &token, |_| {}).unwrap_err();
        assert!(
            matches!(
                error.downcast_ref::<crate::Error>(),
                Some(crate::Error::Canceled(_))
            ) || error.downcast_ref::<import::Cancelled>().is_some(),
            "{error:#}"
        );
        assert!(!paths.environment_config("alpha").exists());
        assert!(!root.path().join("prefixes/alpha").exists());
    }

    #[test]
    fn an_unreadable_playtime_is_reported_not_fatal() {
        let root = tempdir().unwrap();
        let (paths, global, source) = library(root.path());
        fs::write(source.config_dir().join("store/timestamp.json"), "not json").unwrap();
        let plan = plan(&paths, &global, &source);
        let registered = apply(&paths, &global, &source, &plan, &Token::new(), |_| {}).unwrap();
        assert!(matches!(registered.playtime, Playtime::Unreadable(_)));
        assert!(paths.environment_config("alpha").exists());
    }

    #[test]
    fn import_log_appends_a_timestamped_section_each_time() {
        let root = tempdir().unwrap();
        let body = "Heroic game: Game (game)\n  desktop entry: Game\n";
        let paths = paths(root.path());
        let log_path = write_import_log(&paths, "game", body).unwrap();
        assert_eq!(log_path, paths.import_log("game"));
        let contents = fs::read_to_string(&log_path).unwrap();
        assert!(contents.contains("=== "), "{contents}");
        assert!(contents.contains("Heroic game: Game (game)"), "{contents}");

        // A second write appends another timestamped section.
        write_import_log(&paths, "game", body).unwrap();
        let contents = fs::read_to_string(&log_path).unwrap();
        assert_eq!(contents.matches("=== ").count(), 2, "{contents}");
    }

    /// Every file under `dir` with its bytes, to show nothing there changed.
    fn tree(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        let mut files = std::collections::BTreeMap::new();
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(tree(&path));
            } else {
                files.insert(path.clone(), fs::read(&path).unwrap());
            }
        }
        files
    }

    #[test]
    fn a_heroic_poster_is_stored_as_manual_grid_art_that_fetching_keeps() {
        const JPEG: &[u8] = b"\xff\xd8\xff\xe0 poster";
        let root = tempdir().unwrap();
        let (paths, global, source) = library(root.path());
        let cache = source.config_dir().join("images-cache");
        fs::create_dir_all(&cache).unwrap();
        let poster = cache.join("0123abcd");
        fs::write(&poster, JPEG).unwrap();
        let before = tree(source.config_dir());
        let plan = plan(&paths, &global, &source);
        let registered = apply(&paths, &global, &source, &plan, &Token::new(), |_| {}).unwrap();
        let name = registered.environment.name.as_str();

        let stored = store_poster(&paths, name, "a", &poster).unwrap().unwrap();
        assert_eq!(stored.format, art::Format::Jpeg);
        assert_eq!(
            fs::read(paths.art_dir(name).join("grid.jpg")).unwrap(),
            JPEG
        );
        let record = |paths: &Paths| {
            art::ArtConfig::load(&paths.art_dir(name)).unwrap().kinds[&Kind::Grid]
                .source
                .clone()
        };
        assert_eq!(record(&paths), art::Source::Manual("heroic-a.jpg".into()));

        let fetched = art::Downloaded {
            asset: 42,
            bytes: b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec(),
            format: art::Format::Png,
        };
        crate::art_fetch::publish(
            &paths,
            name,
            None,
            Kind::Grid,
            &fetched,
            crate::art_fetch::Mode::Fill,
        )
        .unwrap();
        assert_eq!(record(&paths), art::Source::Manual("heroic-a.jpg".into()));
        assert!(!paths.art_dir(name).join("grid.png").exists());
        assert_eq!(tree(source.config_dir()), before);
    }

    #[test]
    fn a_missing_or_unusable_poster_is_skipped() {
        let root = tempdir().unwrap();
        let (paths, global, source) = library(root.path());
        let plan = plan(&paths, &global, &source);
        let registered = apply(&paths, &global, &source, &plan, &Token::new(), |_| {}).unwrap();
        let name = registered.environment.name.as_str();
        let missing = root.path().join("gone");
        assert_eq!(store_poster(&paths, name, "a", &missing).unwrap(), None);
        // Icons can be ICO, but Grid art can't.
        let ico = root.path().join("icon");
        let mut bytes = vec![0, 0, 1, 0, 1, 0, 16, 16, 0, 0, 1, 0, 32, 0];
        bytes.extend(4u32.to_le_bytes());
        bytes.extend(22u32.to_le_bytes());
        bytes.extend([0; 4]);
        assert_eq!(art::detect(&bytes), Some(art::Format::Ico));
        fs::write(&ico, bytes).unwrap();
        assert_eq!(store_poster(&paths, name, "a", &ico).unwrap(), None);
        let text = root.path().join("text");
        fs::write(&text, b"not an image").unwrap();
        assert_eq!(store_poster(&paths, name, "a", &text).unwrap(), None);
        assert!(!paths.art_dir(name).join(art::ART_FILE).exists());
    }
}
