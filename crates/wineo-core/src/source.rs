//! Where an environment came from, for sorting and filtering the library.

use crate::config::{EnvironmentConfig, Kind};
use crate::emulator::Emulator;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Store {
    Steam,
    Gog,
    Epic,
    Amazon,
}

impl Store {
    pub const ALL: [Self; 4] = [Self::Steam, Self::Gog, Self::Epic, Self::Amazon];

    /// The lowercase id a saved preset names it by. Unlike `label`, it never
    /// changes with the wording on screen.
    pub fn id(self) -> &'static str {
        match self {
            Self::Steam => "steam",
            Self::Gog => "gog",
            Self::Epic => "epic",
            Self::Amazon => "amazon",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|store| store.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Steam => "Steam",
            Self::Gog => "GOG",
            Self::Epic => "Epic",
            Self::Amazon => "Amazon",
        }
    }
}

/// What an environment runs on. An emulated game runs on its emulator,
/// not on Native, though its environment is native.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Platform {
    Native,
    Windows,
    Emulated(Emulator),
}

impl Platform {
    pub const ALL: [Self; 14] = [
        Self::Native,
        Self::Windows,
        Self::Emulated(Emulator::Dolphin),
        Self::Emulated(Emulator::RetroArch),
        Self::Emulated(Emulator::Cemu),
        Self::Emulated(Emulator::Pcsx2),
        Self::Emulated(Emulator::DuckStation),
        Self::Emulated(Emulator::Ppsspp),
        Self::Emulated(Emulator::Rpcs3),
        Self::Emulated(Emulator::MelonDs),
        Self::Emulated(Emulator::Mgba),
        Self::Emulated(Emulator::Xemu),
        Self::Emulated(Emulator::Azahar),
        Self::Emulated(Emulator::Other),
    ];

    /// The lowercase id a saved preset names it by; see `Store::id`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Windows => "windows",
            Self::Emulated(emulator) => emulator.id(),
        }
    }

    /// Exact, so an emulator id this wineo doesn't know is no platform at
    /// all rather than `Emulated(Other)`, whose id is `emulated`.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|platform| platform.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Native => "Native",
            Self::Windows => "Windows",
            Self::Emulated(emulator) => emulator.name(),
        }
    }
}

/// How wineo knows about an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Integration {
    SteamLibrary,
    GogInstall,
    HeroicImport,
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Source {
    pub store: Option<Store>,
    pub integration: Integration,
    pub native: bool,
    /// What it runs on. A proxied Steam game takes it from `Facts` and has
    /// none when Steam doesn't say.
    pub platform: Option<Platform>,
}

/// What a `Source` is decided from.
#[derive(Debug, Clone, Copy, Default)]
pub struct Facts<'a> {
    /// A proxied Steam game.
    pub steam: bool,
    pub native: bool,
    pub gog_id: Option<&'a str>,
    pub heroic_id: Option<&'a str>,
    /// The Wine config's `store`, which only Heroic imports usually set.
    pub store_hint: Option<&'a str>,
    /// What a proxied Steam game runs on, when something shows it (see
    /// `steam::place`). Ignored for every other game.
    pub steam_platform: Option<Platform>,
    /// `[emulation] emulator`, which makes the platform that emulator.
    pub emulator: Option<&'a str>,
}

impl Source {
    /// The word the desktop app shows for it: the store, or failing that
    /// what it runs on. None when neither is known. Core owns it so the
    /// Source column and the Source sort agree.
    pub fn label(&self) -> Option<&'static str> {
        self.store
            .map(Store::label)
            .or_else(|| self.platform.map(Platform::label))
    }

    /// The first matching rule wins: Steam, then a GOG id, then a Heroic
    /// import, then plain.
    pub fn new(facts: Facts<'_>) -> Self {
        let (store, integration) = if facts.steam {
            (Some(Store::Steam), Integration::SteamLibrary)
        } else if facts.gog_id.is_some() {
            let integration = if facts.heroic_id.is_some() {
                Integration::HeroicImport
            } else {
                Integration::GogInstall
            };
            (Some(Store::Gog), integration)
        } else if facts.heroic_id.is_some() {
            let store = match facts.store_hint {
                Some("egs") => Some(Store::Epic),
                Some("gog") => Some(Store::Gog),
                Some("amazon") => Some(Store::Amazon),
                _ => None,
            };
            (store, Integration::HeroicImport)
        } else {
            (None, Integration::Plain)
        };
        let platform = if facts.steam {
            facts.steam_platform
        } else if let Some(emulator) = facts.emulator {
            Some(Platform::Emulated(Emulator::from_config(emulator)))
        } else if facts.native {
            Some(Platform::Native)
        } else {
            Some(Platform::Windows)
        };
        Self {
            store,
            integration,
            native: facts.native,
            platform,
        }
    }

    pub fn of(config: &EnvironmentConfig, steam_platform: Option<Platform>) -> Self {
        Self::new(Facts {
            steam: matches!(config.kind, Kind::Proxied(_)),
            native: matches!(config.kind, Kind::Native),
            gog_id: config.gog_id.as_deref(),
            heroic_id: config.heroic_id.as_deref(),
            store_hint: match &config.kind {
                Kind::Wine(wine) => wine.store.as_deref(),
                _ => None,
            },
            steam_platform,
            emulator: config
                .emulation
                .as_ref()
                .map(|emulation| emulation.emulator.as_str()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(facts: Facts<'_>) -> (Option<Store>, Integration, Option<Platform>) {
        let source = Source::new(facts);
        (source.store, source.integration, source.platform)
    }

    #[test]
    fn steam_wins_over_everything_else() {
        let facts = Facts {
            steam: true,
            gog_id: Some("1"),
            heroic_id: Some("h"),
            store_hint: Some("egs"),
            ..Facts::default()
        };
        assert_eq!(
            source(facts),
            (Some(Store::Steam), Integration::SteamLibrary, None)
        );
    }

    #[test]
    fn a_gog_id_is_a_gog_install() {
        let facts = Facts {
            gog_id: Some("1207658924"),
            ..Facts::default()
        };
        assert_eq!(
            source(facts),
            (
                Some(Store::Gog),
                Integration::GogInstall,
                Some(Platform::Windows)
            )
        );
    }

    #[test]
    fn a_heroic_gog_import_with_a_gog_id_is_gog_through_heroic() {
        let facts = Facts {
            gog_id: Some("1207658924"),
            heroic_id: Some("1207658924"),
            // The id wins even over a hint that says otherwise.
            store_hint: Some("egs"),
            ..Facts::default()
        };
        assert_eq!(
            source(facts),
            (
                Some(Store::Gog),
                Integration::HeroicImport,
                Some(Platform::Windows)
            )
        );
    }

    #[test]
    fn a_heroic_import_takes_its_store_from_the_hint() {
        for (hint, store) in [
            ("egs", Store::Epic),
            ("gog", Store::Gog),
            ("amazon", Store::Amazon),
        ] {
            let facts = Facts {
                heroic_id: Some("id"),
                store_hint: Some(hint),
                ..Facts::default()
            };
            assert_eq!(
                source(facts),
                (
                    Some(store),
                    Integration::HeroicImport,
                    Some(Platform::Windows)
                )
            );
        }
    }

    #[test]
    fn an_unknown_or_missing_hint_means_no_store() {
        for hint in [None, Some("sideload"), Some("EGS"), Some("")] {
            let facts = Facts {
                heroic_id: Some("id"),
                store_hint: hint,
                ..Facts::default()
            };
            assert_eq!(
                source(facts),
                (None, Integration::HeroicImport, Some(Platform::Windows)),
                "{hint:?}"
            );
        }
    }

    #[test]
    fn a_native_heroic_import_has_no_store() {
        let facts = Facts {
            native: true,
            heroic_id: Some("id"),
            ..Facts::default()
        };
        assert_eq!(
            source(facts),
            (None, Integration::HeroicImport, Some(Platform::Native))
        );
    }

    #[test]
    fn a_store_hint_without_a_heroic_id_is_ignored() {
        let facts = Facts {
            store_hint: Some("egs"),
            ..Facts::default()
        };
        assert_eq!(
            source(facts),
            (None, Integration::Plain, Some(Platform::Windows))
        );
    }

    #[test]
    fn everything_else_is_plain_native_or_windows() {
        let native = Facts {
            native: true,
            ..Facts::default()
        };
        assert_eq!(
            source(native),
            (None, Integration::Plain, Some(Platform::Native))
        );
        assert_eq!(
            source(Facts::default()),
            (None, Integration::Plain, Some(Platform::Windows))
        );
    }

    #[test]
    fn a_native_gog_game_is_in_gog_and_native() {
        let facts = Facts {
            native: true,
            gog_id: Some("1207658924"),
            ..Facts::default()
        };
        assert_eq!(
            source(facts),
            (
                Some(Store::Gog),
                Integration::GogInstall,
                Some(Platform::Native)
            )
        );
    }

    #[test]
    fn a_proxied_steam_game_has_the_platform_it_was_given_or_none() {
        for (native, given) in [
            (false, None),
            (true, None),
            (false, Some(Platform::Windows)),
            (false, Some(Platform::Native)),
        ] {
            let facts = Facts {
                steam: true,
                native,
                steam_platform: given,
                ..Facts::default()
            };
            assert_eq!(Source::new(facts).platform, given);
        }
    }

    #[test]
    fn a_steam_platform_is_ignored_for_other_games() {
        let facts = Facts {
            steam_platform: Some(Platform::Native),
            ..Facts::default()
        };
        assert_eq!(Source::new(facts).platform, Some(Platform::Windows));
    }

    #[test]
    fn labels_name_each_store_and_platform() {
        let stores = [Store::Steam, Store::Gog, Store::Epic, Store::Amazon];
        assert_eq!(stores.map(Store::label), ["Steam", "GOG", "Epic", "Amazon"]);
        assert_eq!(
            [Platform::Native, Platform::Windows].map(Platform::label),
            ["Native", "Windows"]
        );
    }

    #[test]
    fn ids_are_lowercase_stable_and_read_back() {
        assert_eq!(
            Store::ALL.map(Store::id),
            ["steam", "gog", "epic", "amazon"]
        );
        assert_eq!(
            Platform::ALL.map(Platform::id),
            [
                "native",
                "windows",
                "dolphin",
                "retroarch",
                "cemu",
                "pcsx2",
                "duckstation",
                "ppsspp",
                "rpcs3",
                "melonds",
                "mgba",
                "xemu",
                "azahar",
                "emulated",
            ]
        );
        for store in Store::ALL {
            assert_eq!(Store::from_id(store.id()), Some(store));
        }
        for platform in Platform::ALL {
            assert_eq!(Platform::from_id(platform.id()), Some(platform));
        }
        // Ids are exact, so a label or a different case isn't one.
        assert_eq!(Store::from_id("GOG"), None);
        assert_eq!(Store::from_id("Steam"), None);
        assert_eq!(Store::from_id(""), None);
        assert_eq!(Platform::from_id("Windows"), None);
        assert_eq!(Store::from_id("windows"), None);
    }

    #[test]
    fn the_shown_label_is_the_store_then_the_platform_then_none() {
        let label = |facts| Source::new(facts).label();
        assert_eq!(
            label(Facts {
                steam: true,
                steam_platform: Some(Platform::Windows),
                ..Facts::default()
            }),
            Some("Steam")
        );
        assert_eq!(
            label(Facts {
                gog_id: Some("1"),
                ..Facts::default()
            }),
            Some("GOG")
        );
        assert_eq!(
            label(Facts {
                native: true,
                ..Facts::default()
            }),
            Some("Native")
        );
        assert_eq!(label(Facts::default()), Some("Windows"));
        // A Heroic import of an unknown store falls back to its platform.
        assert_eq!(
            label(Facts {
                heroic_id: Some("x"),
                store_hint: Some("zoom"),
                ..Facts::default()
            }),
            Some("Windows")
        );
        let none = Source {
            store: None,
            platform: None,
            ..Source::new(Facts::default())
        };
        assert_eq!(none.label(), None);
    }

    #[test]
    fn an_emulated_game_runs_on_its_emulator_not_native() {
        for (id, platform, label) in [
            ("dolphin", Platform::Emulated(Emulator::Dolphin), "Dolphin"),
            (
                "retroarch",
                Platform::Emulated(Emulator::RetroArch),
                "RetroArch",
            ),
            ("custom", Platform::Emulated(Emulator::Other), "Emulated"),
            ("eden", Platform::Emulated(Emulator::Other), "Emulated"),
        ] {
            let source = Source::new(Facts {
                native: true,
                emulator: Some(id),
                ..Facts::default()
            });
            assert_eq!(
                (source.store, source.integration, source.platform),
                (None, Integration::Plain, Some(platform)),
                "{id}"
            );
            assert_eq!(source.label(), Some(label), "{id}");
        }
    }

    #[test]
    fn an_emulated_game_with_a_store_keeps_the_store() {
        let gog = Source::new(Facts {
            native: true,
            emulator: Some("dolphin"),
            gog_id: Some("1207658924"),
            ..Facts::default()
        });
        assert_eq!(
            (gog.store, gog.integration, gog.platform),
            (
                Some(Store::Gog),
                Integration::GogInstall,
                Some(Platform::Emulated(Emulator::Dolphin))
            )
        );
        assert_eq!(gog.label(), Some("GOG"));
        let heroic = Source::new(Facts {
            native: true,
            emulator: Some("retroarch"),
            heroic_id: Some("id"),
            ..Facts::default()
        });
        // A native Heroic import has no store, so its emulator shows.
        assert_eq!(heroic.label(), Some("RetroArch"));
    }

    #[test]
    fn only_a_known_preset_id_names_an_emulator() {
        assert_eq!(
            Platform::from_id("dolphin"),
            Some(Platform::Emulated(Emulator::Dolphin))
        );
        assert_eq!(
            Platform::from_id("emulated"),
            Some(Platform::Emulated(Emulator::Other))
        );
        for id in ["custom", "eden", "Dolphin", "Emulated"] {
            assert_eq!(Platform::from_id(id), None, "{id}");
        }
    }

    #[test]
    fn of_reads_the_emulator_and_a_missing_system_is_fine() {
        let parse = |text: &str| {
            EnvironmentConfig::parse(
                std::path::Path::new("game.toml"),
                &format!("schema_version = 1\nkind = 'native'\n{text}"),
            )
            .unwrap()
        };
        let emulated = parse("[emulation]\nemulator = 'cemu'\nrom = '/roms/a.wua'\n");
        let source = Source::of(&emulated, None);
        assert_eq!(source.platform, Some(Platform::Emulated(Emulator::Cemu)));
        assert_eq!(source.label(), Some("Cemu"));
        let plain = parse("");
        assert_eq!(Source::of(&plain, None).platform, Some(Platform::Native));
    }

    #[test]
    fn of_reads_the_config() {
        let wine = EnvironmentConfig::parse(
            std::path::Path::new("game.toml"),
            "schema_version = 1\nkind = 'wine'\nprefix = 'p'\nrunner = 'system'\narch = 'win64'\n\
             store = 'egs'\nheroic_id = 'abc'\n",
        )
        .unwrap();
        let source = Source::of(&wine, None);
        assert_eq!(
            (source.store, source.integration, source.native),
            (Some(Store::Epic), Integration::HeroicImport, false)
        );
    }
}
