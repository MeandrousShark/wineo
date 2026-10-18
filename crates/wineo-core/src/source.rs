//! Where an environment came from, for sorting and filtering the library.

use crate::config::{EnvironmentConfig, Kind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Store {
    Steam,
    Gog,
    Epic,
    Amazon,
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
}

impl Source {
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
        Self {
            store,
            integration,
            native: facts.native,
        }
    }

    pub fn of(config: &EnvironmentConfig) -> Self {
        Self::new(Facts {
            steam: matches!(config.kind, Kind::Proxied(_)),
            native: matches!(config.kind, Kind::Native),
            gog_id: config.gog_id.as_deref(),
            heroic_id: config.heroic_id.as_deref(),
            store_hint: match &config.kind {
                Kind::Wine(wine) => wine.store.as_deref(),
                _ => None,
            },
        })
    }

    /// The store when there is one, otherwise the platform.
    pub fn label(&self) -> &'static str {
        match self.store {
            Some(Store::Steam) => "Steam",
            Some(Store::Gog) => "GOG",
            Some(Store::Epic) => "Epic",
            Some(Store::Amazon) => "Amazon",
            None if self.native => "Native",
            None => "Windows",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(facts: Facts<'_>) -> (Option<Store>, Integration, &'static str) {
        let source = Source::new(facts);
        (source.store, source.integration, source.label())
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
            (Some(Store::Steam), Integration::SteamLibrary, "Steam")
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
            (Some(Store::Gog), Integration::GogInstall, "GOG")
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
            (Some(Store::Gog), Integration::HeroicImport, "GOG")
        );
    }

    #[test]
    fn a_heroic_import_takes_its_store_from_the_hint() {
        for (hint, store, label) in [
            ("egs", Store::Epic, "Epic"),
            ("gog", Store::Gog, "GOG"),
            ("amazon", Store::Amazon, "Amazon"),
        ] {
            let facts = Facts {
                heroic_id: Some("id"),
                store_hint: Some(hint),
                ..Facts::default()
            };
            assert_eq!(
                source(facts),
                (Some(store), Integration::HeroicImport, label)
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
                (None, Integration::HeroicImport, "Windows"),
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
        assert_eq!(source(facts), (None, Integration::HeroicImport, "Native"));
    }

    #[test]
    fn a_store_hint_without_a_heroic_id_is_ignored() {
        let facts = Facts {
            store_hint: Some("egs"),
            ..Facts::default()
        };
        assert_eq!(source(facts), (None, Integration::Plain, "Windows"));
    }

    #[test]
    fn everything_else_is_plain_native_or_windows() {
        let native = Facts {
            native: true,
            ..Facts::default()
        };
        assert_eq!(source(native), (None, Integration::Plain, "Native"));
        assert_eq!(
            source(Facts::default()),
            (None, Integration::Plain, "Windows")
        );
    }

    #[test]
    fn of_reads_the_config() {
        let wine = EnvironmentConfig::parse(
            std::path::Path::new("game.toml"),
            "schema_version = 1\nkind = 'wine'\nprefix = 'p'\nrunner = 'system'\narch = 'win64'\n\
             store = 'egs'\nheroic_id = 'abc'\n",
        )
        .unwrap();
        let source = Source::of(&wine);
        assert_eq!(
            (source.store, source.integration, source.native),
            (Some(Store::Epic), Integration::HeroicImport, false)
        );
    }
}
