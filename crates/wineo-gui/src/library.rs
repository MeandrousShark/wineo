use std::collections::HashMap;
use std::path::PathBuf;

use iced::Color;
use wineo_core::art::{self, ArtConfig};
use wineo_core::environment::Environment;
use wineo_core::launch;
use wineo_core::paths::Paths;
use wineo_core::report::{self, OverviewFilter};
use wineo_core::runtime::State;
use wineo_core::{stats, time};

use crate::theme::Palette;

#[derive(Debug, Clone, Default)]
pub(super) struct ArtPaths {
    pub(super) hero: Option<PathBuf>,
    pub(super) logo: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub(super) struct Card {
    pub(super) name: String,
    pub(super) state: State,
    pub(super) busy: bool,
    pub(super) busy_holder: Option<String>,
    pub(super) error: Option<String>,
    pub(super) playtime: String,
    pub(super) sessions: usize,
    pub(super) last_played: String,
    pub(super) tags: Vec<String>,
    pub(super) targets: Vec<String>,
    pub(super) default_target: Option<String>,
    pub(super) art: Option<PathBuf>,
    pub(super) panel_art: HashMap<String, ArtPaths>,
}

impl Card {
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

pub(super) fn playtime_label(seconds: i64) -> String {
    if seconds <= 0 {
        "never played".into()
    } else {
        time::duration_hm(seconds)
    }
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
            (name, (summary.seconds, summary.sessions, last))
        })
        .collect();
    let mut cards = Vec::new();
    for item in overview.environments {
        let mut targets = Vec::new();
        let mut default_target = None;
        let mut panel_art = HashMap::new();
        let (error, art) = match Environment::load(paths, &item.name) {
            Ok(env) => {
                targets = env.config.targets.keys().cloned().collect();
                default_target = launch::default_target(&env).ok().map(str::to_owned);
                let dir = paths.art_dir(&item.name);
                let resolved = (|| {
                    let config = ArtConfig::load(&dir)?;
                    for target in &targets {
                        let hero = art::resolve(&dir, &config, Some(target), art::Kind::Hero)
                            .ok()
                            .flatten()
                            .map(|found| found.found.path);
                        let logo = art::resolve(&dir, &config, Some(target), art::Kind::Logo)
                            .ok()
                            .flatten()
                            .map(|found| found.found.path);
                        panel_art.insert(target.clone(), ArtPaths { hero, logo });
                    }
                    Ok::<_, wineo_core::Error>(
                        art::resolve(&dir, &config, default_target.as_deref(), art::Kind::Grid)?
                            .map(|found| found.found.path),
                    )
                })();
                match resolved {
                    Ok(path) => (None, path),
                    Err(error) => (Some(error.to_string()), None),
                }
            }
            Err(error) => (Some(error.to_string()), None),
        };
        let (seconds, sessions, last_played) =
            played
                .get(&item.name)
                .cloned()
                .unwrap_or((0, 0, "never".to_owned()));
        cards.push(Card {
            playtime: playtime_label(seconds),
            sessions,
            last_played,
            tags: item.tags,
            name: item.name,
            state: item.status.state,
            busy: item.busy.is_some() || item.unresolved,
            busy_holder: item.busy.map(|holder| holder.as_str().to_owned()),
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
        let (seconds, sessions, last_played) =
            played
                .get(&error.name)
                .cloned()
                .unwrap_or((0, 0, "never".to_owned()));
        cards.push(Card {
            playtime: playtime_label(seconds),
            sessions,
            last_played,
            tags: Vec::new(),
            name: error.name,
            state: State::Inactive,
            busy: false,
            busy_holder: None,
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
