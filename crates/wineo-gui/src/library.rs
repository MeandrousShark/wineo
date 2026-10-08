use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use iced::Color;
use wineo_core::art::{self, ArtConfig};
use wineo_core::config::{Kind, Target};
use wineo_core::environment::Environment;
use wineo_core::launch;
use wineo_core::paths::Paths;
use wineo_core::report::{self, OverviewFilter};
use wineo_core::runtime::State;
use wineo_core::{stats, time};

use crate::theme::Palette;

#[derive(Debug, Clone, Default)]
pub(super) struct ArtPaths {
    pub(super) hero: Option<ArtFile>,
    pub(super) logo: Option<ArtFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ArtIdentity {
    pub(super) modified: SystemTime,
    pub(super) len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ArtFile {
    pub(super) path: PathBuf,
    pub(super) identity: ArtIdentity,
}

impl ArtFile {
    fn resolve(path: PathBuf) -> Option<Self> {
        let metadata = fs::metadata(&path).ok()?;
        Some(Self {
            path,
            identity: ArtIdentity {
                modified: metadata.modified().ok()?,
                len: metadata.len(),
            },
        })
    }
}

/// What a card's environment runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CardKind {
    Wine,
    Native,
    Proxied,
}

impl CardKind {
    fn new(kind: &Kind) -> Self {
        match kind {
            Kind::Wine(_) => Self::Wine,
            Kind::Native => Self::Native,
            Kind::Proxied(_) => Self::Proxied,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct Card {
    pub(super) name: String,
    /// None when the config can't be read.
    pub(super) kind: Option<CardKind>,
    pub(super) display_title: String,
    pub(super) state: State,
    pub(super) lost: bool,
    pub(super) prefix: Option<PathBuf>,
    pub(super) busy: bool,
    pub(super) busy_holder: Option<wineo_core::operation::Operation>,
    pub(super) unresolved: bool,
    pub(super) error: Option<String>,
    pub(super) playtime: String,
    pub(super) sessions: String,
    pub(super) last_played: String,
    pub(super) tags: Vec<String>,
    pub(super) targets: Vec<String>,
    pub(super) default_target: Option<String>,
    pub(super) art: Option<ArtFile>,
    pub(super) panel_art: HashMap<String, ArtPaths>,
}

impl Card {
    pub(super) fn launch_button_label(&self) -> &'static str {
        if self.tags.iter().any(|tag| tag == "game") {
            "Play"
        } else {
            "Launch"
        }
    }

    pub(super) fn state_word(&self) -> &'static str {
        if self.error.is_some() {
            "error"
        } else if self.busy {
            "busy"
        } else {
            self.state.as_str()
        }
    }
    pub(super) fn dot(&self, palette: Palette) -> Color {
        if self.error.is_some() || self.busy || self.state == State::Failed {
            palette.warning
        } else if self.state == State::Running {
            palette.success
        } else {
            palette.muted
        }
    }
    pub(super) fn playtime(&self) -> &str {
        &self.playtime
    }
}

fn display_title(
    name: &str,
    targets: &BTreeMap<String, Target>,
    default_target: Option<&str>,
) -> String {
    default_target
        .and_then(|target| targets.get(target))
        .filter(|target| target.desktop)
        .and_then(|target| target.name.as_deref())
        .filter(|title| !title.is_empty())
        .unwrap_or(name)
        .to_owned()
}

pub(super) fn playtime_label(seconds: i64) -> String {
    if seconds <= 0 {
        "never played".into()
    } else {
        time::duration_hm(seconds)
    }
}

/// Playtime, sessions, and last played when no stats summary came back.
/// Steam's playtime isn't known then, so it never shows as zero.
fn fallback_stats(kind: Option<CardKind>) -> (String, String, String) {
    let (playtime, sessions, last_played) = if kind == Some(CardKind::Proxied) {
        ("unknown", "-", "unknown")
    } else {
        ("never played", "0", "never")
    };
    (
        playtime.to_owned(),
        sessions.to_owned(),
        last_played.to_owned(),
    )
}

pub(super) fn load_library(paths: &Paths) -> anyhow::Result<Vec<Card>> {
    let overview = report::overview(paths, OverviewFilter::default())?;
    let stats = stats::overview(paths)?;
    let played: HashMap<_, _> = stats
        .rows
        .into_iter()
        .map(|(name, summary)| {
            let last = summary.last_played.map_or_else(
                || "never".to_owned(),
                |when| time::day_label(stats.now, when, &stats.tz),
            );
            let playtime = if summary.playtime_unknown {
                "unknown".to_owned()
            } else {
                playtime_label(summary.seconds)
            };
            let sessions = if summary.source == wineo_core::stats::Source::Steam {
                "-".to_owned()
            } else {
                summary.sessions.to_string()
            };
            (name, (playtime, sessions, last))
        })
        .collect();
    let mut cards = Vec::new();
    for item in overview.environments {
        let mut prefix = None;
        let mut kind = None;
        let mut targets = Vec::new();
        let mut default_target = None;
        let mut title = item.name.clone();
        let mut panel_art = HashMap::new();
        let (error, art) = match Environment::load(paths, &item.name) {
            Ok(env) => {
                kind = Some(CardKind::new(&env.config.kind));
                prefix = env.prefix(paths).ok();
                targets = env.config.targets.keys().cloned().collect();
                default_target = launch::default_target(&env).ok().map(str::to_owned);
                title = display_title(&item.name, &env.config.targets, default_target.as_deref());
                let dir = paths.art_dir(&item.name);
                let resolved = (|| {
                    let config = ArtConfig::load(&dir)?;
                    for target in &targets {
                        let hero = art::resolve(&dir, &config, Some(target), art::Kind::Hero)
                            .ok()
                            .flatten()
                            .and_then(|found| ArtFile::resolve(found.found.path));
                        let logo = art::resolve(&dir, &config, Some(target), art::Kind::Logo)
                            .ok()
                            .flatten()
                            .and_then(|found| ArtFile::resolve(found.found.path));
                        panel_art.insert(target.clone(), ArtPaths { hero, logo });
                    }
                    Ok::<_, wineo_core::Error>(
                        art::resolve(&dir, &config, default_target.as_deref(), art::Kind::Grid)?
                            .and_then(|found| ArtFile::resolve(found.found.path)),
                    )
                })();
                match resolved {
                    Ok(path) => (None, path),
                    Err(error) => (Some(error.to_string()), None),
                }
            }
            Err(error) => (Some(error.to_string()), None),
        };
        let (playtime, sessions, last_played) = played
            .get(&item.name)
            .cloned()
            .unwrap_or_else(|| fallback_stats(kind));
        cards.push(Card {
            playtime,
            sessions,
            last_played,
            tags: item.tags,
            name: item.name,
            kind,
            display_title: title,
            state: item.status.state,
            lost: item.status.lost,
            prefix,
            busy: item.busy.is_some() || item.unresolved,
            busy_holder: item.busy,
            unresolved: item.unresolved,
            error,
            art,
            targets,
            default_target,
            panel_art,
        });
    }
    for error in overview.errors {
        if cards.iter().any(|card| card.name == error.name) {
            continue;
        }
        let (playtime, sessions, last_played) = played
            .get(&error.name)
            .cloned()
            .unwrap_or_else(|| fallback_stats(None));
        let title = error.name.clone();
        cards.push(Card {
            playtime,
            sessions,
            last_played,
            tags: Vec::new(),
            name: error.name,
            kind: None,
            display_title: title,
            state: State::Inactive,
            lost: false,
            prefix: None,
            busy: false,
            busy_holder: None,
            unresolved: false,
            error: Some(error.message),
            art: None,
            targets: Vec::new(),
            default_target: None,
            panel_art: HashMap::new(),
        });
    }
    cards.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(cards)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(desktop: bool, name: Option<&str>) -> Target {
        Target {
            exe: "app.exe".into(),
            args: Vec::new(),
            cwd: None,
            default: false,
            gamemode: None,
            wayland: None,
            mangohud: None,
            gamescope: None,
            hooks: None,
            desktop,
            name: name.map(str::to_owned),
        }
    }

    #[test]
    fn display_title_uses_only_the_default_targets_explicit_desktop_name() {
        let targets = BTreeMap::from([
            ("app".to_owned(), target(true, Some("My App"))),
            ("tool".to_owned(), target(true, Some("Setup Tool"))),
        ]);

        assert_eq!(display_title("env_id", &targets, Some("app")), "My App");
        assert_eq!(
            display_title("env_id", &targets, Some("tool")),
            "Setup Tool"
        );
    }

    #[test]
    fn display_title_falls_back_for_missing_disabled_or_unnamed_targets() {
        let targets = BTreeMap::from([
            ("no-entry".to_owned(), target(false, Some("Ignored"))),
            ("no-title".to_owned(), target(true, None)),
            ("empty-title".to_owned(), target(true, Some(""))),
        ]);

        for default in [
            None,
            Some("missing"),
            Some("no-entry"),
            Some("no-title"),
            Some("empty-title"),
        ] {
            assert_eq!(display_title("env_id", &targets, default), "env_id");
        }
    }

    #[test]
    fn missing_stats_use_the_kind_specific_fallback() {
        let never = ("never played".into(), "0".into(), "never".into());
        assert_eq!(fallback_stats(Some(CardKind::Wine)), never);
        assert_eq!(fallback_stats(Some(CardKind::Native)), never);
        assert_eq!(fallback_stats(None), never);
        assert_eq!(
            fallback_stats(Some(CardKind::Proxied)),
            ("unknown".into(), "-".into(), "unknown".into())
        );
    }

    #[test]
    fn launch_button_label_uses_the_exact_game_tag() {
        let card = |tags: Vec<&str>| Card {
            name: "env_id".into(),
            kind: None,
            display_title: "My App".into(),
            state: State::Inactive,
            lost: false,
            prefix: None,
            busy: false,
            busy_holder: None,
            unresolved: false,
            error: None,
            playtime: "never played".into(),
            sessions: "0".to_owned(),
            last_played: "never".into(),
            tags: tags.into_iter().map(str::to_owned).collect(),
            targets: Vec::new(),
            default_target: None,
            art: None,
            panel_art: HashMap::new(),
        };

        assert_eq!(card(vec!["game"]).launch_button_label(), "Play");
        assert_eq!(card(vec!["Game"]).launch_button_label(), "Launch");
        assert_eq!(card(vec!["work"]).launch_button_label(), "Launch");
    }

    #[test]
    fn cards_know_their_kind() {
        let root = tempfile::tempdir().unwrap();
        let paths =
            Paths::from_lookup(|key| (key == "HOME").then(|| root.path().as_os_str().to_owned()))
                .unwrap();
        fs::create_dir_all(paths.environments_dir()).unwrap();
        for (name, text) in [
            (
                "bg3",
                "kind = 'proxied'\nlauncher = 'steam'\napp_id = '1086940'\n",
            ),
            ("dusk", "kind = 'native'\n"),
            (
                "notes",
                "prefix = '/tmp/notes'\nrunner = 'wine'\narch = 'win64'\n",
            ),
            ("broken", "not toml ["),
        ] {
            fs::write(paths.environment_config(name), text).unwrap();
        }
        let cards = load_library(&paths).unwrap();
        let kind = |name: &str| cards.iter().find(|card| card.name == name).unwrap().kind;
        assert_eq!(kind("bg3"), Some(CardKind::Proxied));
        assert_eq!(kind("dusk"), Some(CardKind::Native));
        assert_eq!(kind("notes"), Some(CardKind::Wine));
        assert_eq!(kind("broken"), None);
        let bg3 = cards.iter().find(|card| card.name == "bg3").unwrap();
        assert!(bg3.prefix.is_none());
        assert_eq!(bg3.default_target.as_deref(), Some("steam"));
    }

    #[test]
    fn missing_art_file_has_no_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grid.png");
        fs::write(&path, b"first").unwrap();
        let art = ArtFile::resolve(path.clone()).unwrap();
        assert_eq!(art.identity.len, 5);
        fs::remove_file(&path).unwrap();
        assert!(ArtFile::resolve(path).is_none());
    }
}
