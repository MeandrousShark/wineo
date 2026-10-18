use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use iced::Color;
use jiff::Timestamp;
use wineo_core::art::{self, ArtConfig};
use wineo_core::config::{Kind, Target};
use wineo_core::environment::Environment;
use wineo_core::launch;
use wineo_core::paths::Paths;
use wineo_core::report::{self, OverviewFilter};
use wineo_core::runtime::State;
use wineo_core::source::{Facts, Source};
use wineo_core::trash::PendingScan;
use wineo_core::{stats, steam, time};

use crate::theme::Palette;

#[derive(Debug, Clone, Default, PartialEq)]
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

#[derive(Debug, Clone, PartialEq)]
pub(super) struct SteamDetails {
    pub(super) install: report::InstallReport,
    pub(super) proton: Option<String>,
    pub(super) size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Card {
    pub(super) name: String,
    /// None when the config can't be read.
    pub(super) kind: Option<CardKind>,
    pub(super) display_title: String,
    pub(super) steam: Option<SteamDetails>,
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
    /// Raw values for sorting. Unlike the labels above they never depend on
    /// today's date, so a quiet rescan finds no change.
    pub(super) seconds: i64,
    pub(super) playtime_unknown: bool,
    pub(super) last_played_at: Option<Timestamp>,
    /// From `library.toml`.
    pub(super) added: Option<Timestamp>,
    pub(super) favorite: bool,
    pub(super) hidden: bool,
    pub(super) source: Source,
    pub(super) tags: Vec<String>,
    pub(super) targets: Vec<String>,
    pub(super) default_target: Option<String>,
    pub(super) art: Option<ArtFile>,
    pub(super) panel_art: HashMap<String, ArtPaths>,
    /// GOG facts read from disk: whether it is a GOG game, and whether an
    /// interrupted update stops it from launching.
    pub(super) gog: Option<crate::gog_library::GogCard>,
}

impl Card {
    pub(super) fn launch_button_label(&self) -> &'static str {
        if self.tags.iter().any(|tag| tag == "game") {
            "Play"
        } else {
            "Launch"
        }
    }

    pub(super) fn active(&self) -> bool {
        matches!(
            self.state,
            State::Starting | State::Running | State::SteamRunning
        )
    }

    pub(super) fn not_installed(&self) -> bool {
        self.steam
            .as_ref()
            // A queued install has nothing on disk yet, so it dims like a game
            // Steam doesn't have.
            .is_some_and(|steam| matches!(steam.install.state, "not_installed" | "install_queued"))
    }

    /// Install takes Play's place for a game Steam doesn't have on disk.
    pub(super) fn installable(&self) -> bool {
        self.not_installed()
    }

    /// Only games Steam has fully installed offer Uninstall.
    pub(super) fn uninstallable(&self) -> bool {
        self.steam
            .as_ref()
            .is_some_and(|steam| matches!(steam.install.state, "installed" | "updating"))
    }

    /// How far Steam's install is, for the poster's bar.
    pub(super) fn install_progress(&self) -> Option<u8> {
        self.steam
            .as_ref()
            .filter(|steam| steam.install.state == "installing")
            .and_then(|steam| steam.install.progress)
    }

    pub(super) fn install_allows_play(&self) -> bool {
        self.steam
            .as_ref()
            .is_none_or(|steam| matches!(steam.install.state, "installed" | "updating"))
    }

    /// Whether an interrupted GOG update stops this from launching: its own,
    /// or another game's whose folder it runs from.
    pub(super) fn gog_blocks_play(&self) -> bool {
        self.gog
            .as_ref()
            .is_some_and(|gog| gog.unfinished || gog.blocked_by.is_some())
    }

    /// Finish update takes Play's place for the owner of a GOG game whose
    /// update was interrupted.
    pub(super) fn finish_update(&self) -> bool {
        self.gog.as_ref().is_some_and(|gog| gog.unfinished)
    }

    pub(super) fn install_warning(&self) -> Option<&str> {
        self.steam
            .as_ref()
            .filter(|steam| matches!(steam.install.state, "unavailable" | "unknown"))
            .map(|steam| {
                steam
                    .install
                    .problem
                    .as_deref()
                    .unwrap_or(steam.install.display())
            })
    }

    pub(super) fn state_word(&self) -> &'static str {
        if self.error.is_some() {
            "error"
        } else if self.busy {
            "busy"
        } else if !self.active()
            && let Some(steam) = &self.steam
            && steam.install.state != "installed"
        {
            steam.install.display()
        } else {
            self.state.as_str()
        }
    }

    /// `state_word` with an install's percent, for the panel.
    pub(super) fn state_label(&self) -> String {
        match &self.steam {
            Some(steam) if self.state_word() == steam.install.display() => steam.install.label(),
            _ => self.state_word().to_owned(),
        }
    }
    pub(super) fn dot(&self, palette: Palette) -> Color {
        if self.error.is_some()
            || self.busy
            || self.state == State::Failed
            || self.install_warning().is_some()
        {
            palette.warning
        } else if self.active() {
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

/// A card's playtime fields, as labels and raw values.
#[derive(Clone)]
struct Played {
    playtime: String,
    sessions: String,
    last_played: String,
    seconds: i64,
    playtime_unknown: bool,
    last_played_at: Option<Timestamp>,
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

/// `fallback_stats` with the raw values: never played, or unknown for Steam.
fn fallback_played(kind: Option<CardKind>) -> Played {
    let (playtime, sessions, last_played) = fallback_stats(kind);
    Played {
        playtime,
        sessions,
        last_played,
        seconds: 0,
        playtime_unknown: kind == Some(CardKind::Proxied),
        last_played_at: None,
    }
}

pub(super) fn load_library(paths: &Paths) -> anyhow::Result<Vec<Card>> {
    load_library_with(paths, &wineo_core::library::load(paths), None)
}

/// `load_library` with `library.toml` already read and, when given, the
/// stats logs read through a cache, so a caller that reads often can skip
/// what didn't change.
pub(super) fn load_library_with(
    paths: &Paths,
    saved: &wineo_core::library::Library,
    stats: Option<&mut stats::Cache>,
) -> anyhow::Result<Vec<Card>> {
    // Building cards never trashes or restores, so the overview and every
    // card's config load share one read of the trash.
    let pending = PendingScan::new(paths);
    let overview = report::overview_with_pending(paths, OverviewFilter::default(), &pending)?;
    let tools = overview
        .steam
        .as_ref()
        .and_then(|steam| steam.install.as_ref())
        .map(steam::compat_tools)
        .unwrap_or_default();
    let stats = match stats {
        Some(cache) => stats::overview_cached(paths, cache)?,
        None => stats::overview(paths)?,
    };
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
            let played = Played {
                playtime,
                sessions,
                last_played: last,
                seconds: summary.seconds,
                playtime_unknown: summary.playtime_unknown,
                last_played_at: summary.last_played,
            };
            (name, played)
        })
        .collect();
    let gog = crate::gog_library::Folders::read(paths);
    let mut cards = Vec::new();
    for item in overview.environments {
        let steam = if let Some(report::KindReport::Proxied {
            app_id, install, ..
        }) = &item.kind
        {
            Some(SteamDetails {
                install: install.clone(),
                proton: steam::compat_tool_in(&tools, app_id),
                size: overview
                    .steam
                    .as_ref()
                    .and_then(|steam| steam.manifests.iter().find(|entry| entry.app_id == *app_id))
                    .and_then(|entry| entry.manifest.as_ref().ok())
                    .map(|manifest| manifest.size_on_disk),
            })
        } else {
            None
        };
        let mut prefix = None;
        let mut kind = None;
        let mut targets = Vec::new();
        let mut default_target = None;
        let mut title = item.name.clone();
        let mut panel_art = HashMap::new();
        let mut gog_card = None;
        // The overview's kind, for when the config can't be loaded here.
        let mut source = Source::new(Facts {
            steam: matches!(item.kind, Some(report::KindReport::Proxied { .. })),
            native: matches!(item.kind, Some(report::KindReport::Native)),
            ..Facts::default()
        });
        let (error, art) = match Environment::load_with_pending(paths, &item.name, &pending) {
            Ok(env) => {
                kind = Some(CardKind::new(&env.config.kind));
                source = Source::of(&env.config);
                gog_card = gog.card(paths, &env);
                prefix = env.prefix(paths).ok();
                targets = env.config.targets.keys().cloned().collect();
                default_target = launch::default_target(&env).ok().map(str::to_owned);
                title = display_title(&item.name, &env.config.targets, default_target.as_deref());
                if let Some(report::KindReport::Proxied {
                    title: live_title, ..
                }) = &item.kind
                {
                    title = live_title
                        .clone()
                        .filter(|title| !title.is_empty())
                        .unwrap_or_else(|| item.name.clone());
                    targets = vec!["steam".to_owned()];
                }
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
        let played = played
            .get(&item.name)
            .cloned()
            .unwrap_or_else(|| fallback_played(kind));
        let entry = saved.environments.get(&item.name);
        cards.push(Card {
            playtime: played.playtime,
            sessions: played.sessions,
            last_played: played.last_played,
            seconds: played.seconds,
            playtime_unknown: played.playtime_unknown,
            last_played_at: played.last_played_at,
            added: entry.and_then(|entry| entry.added),
            favorite: entry.is_some_and(|entry| entry.favorite),
            hidden: entry.is_some_and(|entry| entry.hidden),
            source,
            tags: item.tags,
            name: item.name,
            kind,
            display_title: title,
            steam,
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
            gog: gog_card,
        });
    }
    for error in overview.errors {
        if cards.iter().any(|card| card.name == error.name) {
            continue;
        }
        let played = played
            .get(&error.name)
            .cloned()
            .unwrap_or_else(|| fallback_played(None));
        let entry = saved.environments.get(&error.name);
        let title = error.name.clone();
        cards.push(Card {
            playtime: played.playtime,
            sessions: played.sessions,
            last_played: played.last_played,
            seconds: played.seconds,
            playtime_unknown: played.playtime_unknown,
            last_played_at: played.last_played_at,
            added: entry.and_then(|entry| entry.added),
            favorite: entry.is_some_and(|entry| entry.favorite),
            hidden: entry.is_some_and(|entry| entry.hidden),
            source: Source::new(Facts::default()),
            tags: Vec::new(),
            name: error.name,
            kind: None,
            display_title: title,
            steam: None,
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
            gog: None,
        });
    }
    cards.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(cards)
}

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;
