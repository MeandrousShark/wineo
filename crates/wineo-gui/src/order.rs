//! The library's shown order. `view` runs every frame during card motion and
//! can't change anything, so the order is rebuilt in `update` whenever what
//! it depends on changes, and every reader takes the cached indices.

use std::borrow::Cow;
use std::collections::BTreeSet;

use wineo_core::library::{self, SortFields, SortKey};
use wineo_core::source::{Platform, Store};

use crate::library::Card;

/// How the library is sorted. Search stays `App::search`, which most of the
/// app reads directly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Query {
    pub(crate) sort: SortKey,
    pub(crate) descending: bool,
    pub(crate) favorites_first: bool,
}

impl From<library::View> for Query {
    fn from(view: library::View) -> Self {
        Self {
            sort: view.sort,
            descending: view.descending,
            favorites_first: view.favorites_first,
        }
    }
}

/// What narrows the library besides search. None of it is saved, so each
/// launch starts with the whole library.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Filters {
    /// Store labels; a card matches any of them. A game with no store
    /// matches none.
    pub(crate) stores: BTreeSet<&'static str>,
    /// Platform labels, the same way. A game with no platform matches none.
    pub(crate) platforms: BTreeSet<&'static str>,
    /// A card matches when it has any of these.
    pub(crate) tags: BTreeSet<String>,
    pub(crate) show_hidden: bool,
}

impl Filters {
    /// How many store, platform, and tag filters are on, for the Filters
    /// button.
    pub(crate) fn active(&self) -> usize {
        self.stores.len() + self.platforms.len() + self.tags.len()
    }

    /// Any checked value within a group matches, and every group with a
    /// check has to.
    fn admit(&self, card: &Card) -> bool {
        (self.stores.is_empty()
            || card
                .source
                .store
                .is_some_and(|store| self.stores.contains(store.label())))
            && (self.platforms.is_empty()
                || card
                    .source
                    .platform
                    .is_some_and(|platform| self.platforms.contains(platform.label())))
            && (self.tags.is_empty() || card.tags.iter().any(|tag| self.tags.contains(tag)))
    }
}

/// What the empty states read, counted by the rebuild.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) shown: usize,
    /// Hidden cards the search and filters would show with Show hidden on.
    /// Zero while it's on, since they're shown then.
    pub(crate) hidden_matching: usize,
    pub(crate) hidden: usize,
}

/// The stores, platforms, and tags the library has, for the filter popup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Options {
    /// Store labels, A to Z.
    pub(crate) stores: Vec<&'static str>,
    /// Platform labels, A to Z.
    pub(crate) platforms: Vec<&'static str>,
    /// Tags, A to Z ignoring case.
    pub(crate) tags: Vec<String>,
}

impl Options {
    fn of(cards: &[Card]) -> Self {
        let labels = |label: fn(&Card) -> Option<&'static str>| {
            let mut labels: Vec<&'static str> = cards
                .iter()
                .filter_map(label)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            labels.sort_unstable_by_key(|label| label.to_lowercase());
            labels
        };
        let stores = labels(|card| card.source.store.map(Store::label));
        let platforms = labels(|card| card.source.platform.map(Platform::label));
        let mut tags: Vec<String> = cards
            .iter()
            .flat_map(|card| card.tags.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        tags.sort_by_cached_key(|tag| (tag.to_lowercase(), tag.clone()));
        Self {
            stores,
            platforms,
            tags,
        }
    }
}

/// A rebuild's result: the shown indices and what it counted on the way.
#[derive(Debug, Default)]
pub(crate) struct Built {
    indices: Vec<usize>,
    counts: Counts,
}

impl Built {
    pub(crate) fn indices(&self) -> &[usize] {
        &self.indices
    }
}

#[derive(Debug, Default)]
pub(crate) struct Order {
    /// Indices into the cards, in shown order.
    indices: Vec<usize>,
    counts: Counts,
    options: Options,
    /// What the order was built from. Every change goes through
    /// `App::reorder`, so in the app this always matches; tests that set
    /// the cards or search directly get an order computed on the spot.
    built_from: Inputs,
    #[cfg(test)]
    pub(crate) rebuilds: usize,
    /// Reads that found the cache didn't fit and sorted on the spot.
    #[cfg(test)]
    pub(crate) misses: std::cell::Cell<usize>,
}

#[derive(Debug, Default, PartialEq)]
struct Inputs {
    /// Where the cards live and how many there are. Cards are only ever
    /// replaced as a whole, which moves them.
    cards: (usize, usize),
    search: String,
    query: Query,
    filters: Filters,
}

/// Everything the shown order depends on, borrowed from the app.
#[derive(Clone, Copy)]
pub(crate) struct Of<'a> {
    pub(crate) cards: &'a [Card],
    pub(crate) search: &'a str,
    pub(crate) query: Query,
    pub(crate) filters: &'a Filters,
}

impl Of<'_> {
    fn cards_key(&self) -> (usize, usize) {
        (self.cards.as_ptr() as usize, self.cards.len())
    }
}

impl Inputs {
    fn of(of: Of<'_>) -> Self {
        Self {
            cards: of.cards_key(),
            search: of.search.to_owned(),
            query: of.query,
            filters: of.filters.clone(),
        }
    }

    fn fit(&self, of: Of<'_>) -> bool {
        self.cards == of.cards_key()
            && self.search == of.search
            && self.query == of.query
            && self.filters == *of.filters
    }
}

impl Order {
    /// The shown indices for `of`, with the empty states' counts. Hidden
    /// cards go unless Show hidden is on, then the store, platform, and tag filters
    /// and the search narrow, and what's left is sorted. Sort keys are
    /// computed once here, not per comparison.
    pub(crate) fn sorted(of: Of<'_>) -> Built {
        let Of {
            cards,
            search,
            query,
            filters,
        } = of;
        let needle = search.to_lowercase();
        let titles: Vec<String> = cards
            .iter()
            .map(|card| card.display_title.to_lowercase())
            .collect();
        let mut counts = Counts::default();
        let mut indices: Vec<usize> = Vec::new();
        for (index, card) in cards.iter().enumerate() {
            counts.hidden += usize::from(card.hidden);
            let found = filters.admit(card)
                && (titles[index].contains(&needle)
                    || card.name.to_lowercase().contains(&needle)
                    || card
                        .tags
                        .iter()
                        .any(|tag| tag.to_lowercase().contains(&needle)));
            if !found {
                continue;
            }
            if card.hidden && !filters.show_hidden {
                counts.hidden_matching += 1;
            } else {
                indices.push(index);
            }
        }
        counts.shown = indices.len();
        let fields: Vec<SortFields<'_>> = cards
            .iter()
            .zip(&titles)
            .map(|(card, title)| SortFields {
                title,
                name: &card.name,
                seconds: card.seconds,
                playtime_unknown: card.playtime_unknown,
                last_played: card.last_played_at,
                added: card.added,
                favorite: card.favorite,
            })
            .collect();
        indices.sort_unstable_by(|&a, &b| {
            library::compare(
                &fields[a],
                &fields[b],
                query.sort,
                query.descending,
                query.favorites_first,
            )
        });
        Built { indices, counts }
    }

    pub(crate) fn rebuild(&mut self, of: Of<'_>) {
        self.replace(Self::sorted(of), of);
    }

    /// Takes what `sorted` built for exactly these inputs.
    pub(crate) fn replace(&mut self, built: Built, of: Of<'_>) {
        self.indices = built.indices;
        self.counts = built.counts;
        #[cfg(test)]
        {
            self.rebuilds += 1;
        }
        // A search, sort, or filter change keeps the cards and what they
        // offer.
        if self.built_from.cards != of.cards_key() {
            self.options = Options::of(of.cards);
        }
        self.keep(of);
    }

    /// Records that the order still fits new cards whose sort, filter, and
    /// search inputs match the old ones card for card, so their stores,
    /// platforms, and tags are the same too.
    pub(crate) fn keep(&mut self, of: Of<'_>) {
        self.built_from = Inputs::of(of);
    }

    fn fits(&self, of: Of<'_>) -> bool {
        if self.built_from.fit(of) {
            return true;
        }
        // Only tests set the cards or search without `App::reorder`, so a
        // debug run of the app stops here rather than sort every frame.
        #[cfg(all(debug_assertions, not(test)))]
        debug_assert!(false, "the library changed without App::reorder");
        #[cfg(test)]
        self.misses.set(self.misses.get() + 1);
        false
    }

    pub(crate) fn shown<'a>(&self, of: Of<'a>) -> Vec<&'a Card> {
        let cards = of.cards;
        if self.fits(of) {
            return self
                .indices
                .iter()
                .filter_map(|&index| cards.get(index))
                .collect();
        }
        Self::sorted(of)
            .indices
            .into_iter()
            .map(|index| &cards[index])
            .collect()
    }

    pub(crate) fn counts(&self, of: Of<'_>) -> Counts {
        if self.fits(of) {
            self.counts
        } else {
            Self::sorted(of).counts
        }
    }

    pub(crate) fn options(&self, of: Of<'_>) -> Cow<'_, Options> {
        if self.fits(of) {
            Cow::Borrowed(&self.options)
        } else {
            Cow::Owned(Options::of(of.cards))
        }
    }
}

/// Whether two versions of a card would sort, filter, and match a search
/// alike.
pub(crate) fn same_inputs(a: &Card, b: &Card) -> bool {
    a.name == b.name
        && a.display_title == b.display_title
        && a.seconds == b.seconds
        && a.playtime_unknown == b.playtime_unknown
        && a.last_played_at == b.last_played_at
        && a.added == b.added
        && a.favorite == b.favorite
        && a.hidden == b.hidden
        && a.source == b.source
        && a.tags == b.tags
}
