//! The library's shown order. `view` runs every frame during card motion and
//! can't change anything, so the order is rebuilt in `update` whenever what
//! it depends on changes, and every reader takes the cached indices.

use wineo_core::library::{self, SortFields, SortKey};

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

#[derive(Debug, Default)]
pub(crate) struct Order {
    /// Indices into the cards, in shown order.
    indices: Vec<usize>,
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
}

impl Inputs {
    fn of(cards: &[Card], search: &str, query: Query) -> Self {
        Self {
            cards: (cards.as_ptr() as usize, cards.len()),
            search: search.to_owned(),
            query,
        }
    }

    fn fit(&self, cards: &[Card], search: &str, query: Query) -> bool {
        self.cards == (cards.as_ptr() as usize, cards.len())
            && self.search == search
            && self.query == query
    }
}

impl Order {
    /// The shown indices for `cards`. Sort keys are computed once here, not
    /// per comparison.
    pub(crate) fn sorted(cards: &[Card], search: &str, query: Query) -> Vec<usize> {
        let needle = search.to_lowercase();
        let titles: Vec<String> = cards
            .iter()
            .map(|card| card.display_title.to_lowercase())
            .collect();
        let mut indices: Vec<usize> = (0..cards.len())
            .filter(|&index| {
                titles[index].contains(&needle)
                    || cards[index].name.to_lowercase().contains(&needle)
            })
            .collect();
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
                source: card.source.label(),
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
        indices
    }

    pub(crate) fn rebuild(&mut self, cards: &[Card], search: &str, query: Query) {
        self.replace(Self::sorted(cards, search, query), cards, search, query);
    }

    /// Takes indices `sorted` built for exactly these inputs.
    pub(crate) fn replace(
        &mut self,
        indices: Vec<usize>,
        cards: &[Card],
        search: &str,
        query: Query,
    ) {
        self.indices = indices;
        #[cfg(test)]
        {
            self.rebuilds += 1;
        }
        self.keep(cards, search, query);
    }

    /// Records that the order still fits new cards whose sort and search
    /// inputs match the old ones card for card.
    pub(crate) fn keep(&mut self, cards: &[Card], search: &str, query: Query) {
        self.built_from = Inputs::of(cards, search, query);
    }

    pub(crate) fn shown<'a>(&self, cards: &'a [Card], search: &str, query: Query) -> Vec<&'a Card> {
        if self.built_from.fit(cards, search, query) {
            return self
                .indices
                .iter()
                .filter_map(|&index| cards.get(index))
                .collect();
        }
        #[cfg(test)]
        self.misses.set(self.misses.get() + 1);
        Self::sorted(cards, search, query)
            .into_iter()
            .map(|index| &cards[index])
            .collect()
    }
}

/// Whether two versions of a card would sort and match a search alike.
pub(crate) fn same_inputs(a: &Card, b: &Card) -> bool {
    a.name == b.name
        && a.display_title == b.display_title
        && a.seconds == b.seconds
        && a.playtime_unknown == b.playtime_unknown
        && a.last_played_at == b.last_played_at
        && a.added == b.added
        && a.favorite == b.favorite
        && a.source == b.source
}
