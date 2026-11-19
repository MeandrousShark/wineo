//! The library's shown order. `view` runs every frame during card motion and
//! can't change anything, so the order is rebuilt in `update` whenever what
//! it depends on changes, and every reader takes the cached indices.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use wineo_core::library::{self, GroupBy, SortFields, SortKey};
use wineo_core::search::{Field, Search};
use wineo_core::source::{Platform, Store};

use crate::library::Card;
use crate::slot::SectionKey;

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
/// launch starts with the whole library. A value is in at most one of its
/// include and exclude sets; the popup keeps it that way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Filters {
    /// A card matches any of these stores. A game with no store matches
    /// none.
    pub(crate) stores: BTreeSet<Store>,
    /// Platforms, the same way. A game with no platform matches none.
    pub(crate) platforms: BTreeSet<Platform>,
    /// A card matches when it has any of these.
    pub(crate) tags: BTreeSet<String>,
    /// A card with any of these stores is out. A game with no store stays in.
    pub(crate) not_stores: BTreeSet<Store>,
    /// Platforms that take a card out. A game with no platform isn't known
    /// to be on any, so it stays in.
    pub(crate) not_platforms: BTreeSet<Platform>,
    /// A card with any of these tags is out.
    pub(crate) not_tags: BTreeSet<String>,
    pub(crate) show_hidden: bool,
}

impl Filters {
    /// How many store, platform, and tag filters are on, included or
    /// excluded, for the Filters button.
    pub(crate) fn active(&self) -> usize {
        self.stores.len()
            + self.platforms.len()
            + self.tags.len()
            + self.not_stores.len()
            + self.not_platforms.len()
            + self.not_tags.len()
    }

    /// Turns every store, platform, and tag filter off, leaving Show hidden.
    pub(crate) fn clear(&mut self) {
        self.stores.clear();
        self.platforms.clear();
        self.tags.clear();
        self.not_stores.clear();
        self.not_platforms.clear();
        self.not_tags.clear();
    }

    /// Any checked value within a group matches, every group with a check
    /// has to, and no excluded value in any group may match.
    fn admit(&self, card: &Card) -> bool {
        let (store, platform) = (card.source.store, card.source.platform);
        (self.stores.is_empty() || store.is_some_and(|store| self.stores.contains(&store)))
            && (self.platforms.is_empty()
                || platform.is_some_and(|platform| self.platforms.contains(&platform)))
            && (self.tags.is_empty() || card.tags.iter().any(|tag| self.tags.contains(tag)))
            && !store.is_some_and(|store| self.not_stores.contains(&store))
            && !platform.is_some_and(|platform| self.not_platforms.contains(&platform))
            && !card.tags.iter().any(|tag| self.not_tags.contains(tag))
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
    /// Stores, A to Z by label.
    pub(crate) stores: Vec<Store>,
    /// Platforms, A to Z by label.
    pub(crate) platforms: Vec<Platform>,
    /// Tags, A to Z ignoring case.
    pub(crate) tags: Vec<String>,
}

impl Options {
    fn of(cards: &[Card]) -> Self {
        let mut stores: Vec<Store> = cards
            .iter()
            .filter_map(|card| card.source.store)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        stores.sort_unstable_by_key(|store| store.label().to_lowercase());
        let mut platforms: Vec<Platform> = cards
            .iter()
            .filter_map(|card| card.source.platform)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        platforms.sort_unstable_by_key(|platform| platform.label().to_lowercase());
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

/// One section of the shown order, with the indices of its cards in shown
/// order. A collapsed section keeps them, for its count, but draws none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Section {
    pub(crate) key: SectionKey,
    pub(crate) cards: Vec<usize>,
    pub(crate) collapsed: bool,
}

/// A rebuild's result: the shown indices and what it counted on the way.
#[derive(Debug, Default)]
pub(crate) struct Built {
    indices: Vec<usize>,
    counts: Counts,
    sections: Vec<Section>,
    /// Set when the rebuild had to prepare its cards' fields itself.
    prepared: Option<Prepared>,
}

impl Built {
    pub(crate) fn indices(&self) -> &[usize] {
        &self.indices
    }

    /// How many cards' search fields this rebuild made.
    #[cfg(test)]
    pub(crate) fn prepared_cards(&self) -> usize {
        self.prepared.as_ref().map_or(0, |prepared| prepared.made)
    }

    /// Whether these sections, of `cards`, hold the same games in the same
    /// order as `old`, of `old_cards`, so no slot moved.
    pub(crate) fn same_sections(
        &self,
        old: &[Section],
        old_cards: &[Card],
        cards: &[Card],
    ) -> bool {
        let names = |section: &Section, cards: &[Card]| -> Vec<String> {
            section
                .cards
                .iter()
                .filter_map(|&index| cards.get(index))
                .map(|card| card.name.clone())
                .collect()
        };
        self.sections.len() == old.len()
            && self.sections.iter().zip(old).all(|(new, old)| {
                new.key == old.key
                    && new.collapsed == old.collapsed
                    && names(new, cards) == names(old, old_cards)
            })
    }
}

/// What searching and sorting read from each card, made once per set of
/// cards so a keystroke only scans them.
#[derive(Debug)]
pub(crate) struct Prepared {
    /// The cards these were made from, as `Inputs::cards`.
    cards: (usize, usize),
    /// The Name sort key and every tiebreak: the title lowercased, nothing
    /// more, so `Éclair` still sorts after Z.
    titles: Vec<String>,
    /// Made only once a search needs them, so a library nobody searches,
    /// like the tray's, never pays for them.
    fields: Option<Vec<Fields>>,
    /// How many cards' fields were made for this, rather than carried over.
    #[cfg(test)]
    made: usize,
    /// What each card was when these were made, to catch a test that edits
    /// cards in place without `cards_edited`. Benches build with tests but
    /// without debug assertions, so they time the app's code.
    #[cfg(all(test, debug_assertions))]
    made_from: Vec<(String, String, Vec<String>)>,
}

#[derive(Debug)]
struct Fields {
    title: Field,
    name: Field,
    tags: Vec<Field>,
}

impl Fields {
    fn of(card: &Card) -> Self {
        Self {
            title: Field::new(&card.display_title),
            name: Field::new(&card.name),
            tags: card.tags.iter().map(|tag| Field::new(tag)).collect(),
        }
    }

    /// A search never mixes fields, so each is matched on its own.
    fn matched(&self, search: &Search) -> bool {
        search.matches(&self.title)
            || search.matches(&self.name)
            || self.tags.iter().any(|tag| search.matches(tag))
    }
}

/// Whether a card's search fields and sort title would come out the same.
fn same_fields(a: &Card, b: &Card) -> bool {
    a.name == b.name && a.display_title == b.display_title && a.tags == b.tags
}

fn cards_key(cards: &[Card]) -> (usize, usize) {
    (cards.as_ptr() as usize, cards.len())
}

impl Prepared {
    fn of(cards: &[Card], fields: bool) -> Self {
        Self {
            cards: cards_key(cards),
            titles: cards
                .iter()
                .map(|card| card.display_title.to_lowercase())
                .collect(),
            fields: fields.then(|| cards.iter().map(Fields::of).collect()),
            #[cfg(test)]
            made: if fields { cards.len() } else { 0 },
            #[cfg(all(test, debug_assertions))]
            made_from: cards.iter().map(made_from).collect(),
        }
    }

    fn fit(&self, cards: &[Card]) -> bool {
        let fit = self.cards == cards_key(cards);
        #[cfg(all(test, debug_assertions))]
        assert!(
            !fit || self.made_from.iter().zip(cards).all(|(made, card)| {
                made.0 == card.display_title && made.1 == card.name && made.2 == card.tags
            }),
            "a card's title, name, or tags changed in place without Order::cards_edited"
        );
        fit
    }

    /// These, made from `old`, moved over to `cards`: a card whose name,
    /// title, and tags are unchanged keeps what it had, and only the rest
    /// are made again.
    fn carried(mut self, old: &[Card], cards: &[Card]) -> Self {
        // Rescans list cards by name, so most sit where they were.
        let mut by_name: Option<std::collections::HashMap<&str, usize>> = None;
        let mut at = |index: usize, card: &Card| -> Option<usize> {
            if old.get(index).is_some_and(|old| old.name == card.name) {
                return Some(index);
            }
            by_name
                .get_or_insert_with(|| {
                    old.iter()
                        .enumerate()
                        .map(|(index, card)| (card.name.as_str(), index))
                        .collect()
                })
                .get(card.name.as_str())
                .copied()
        };
        let reused: Vec<Option<usize>> = cards
            .iter()
            .enumerate()
            .map(|(index, card)| at(index, card).filter(|&from| same_fields(&old[from], card)))
            .collect();
        let mut titles: Vec<Option<String>> = self.titles.drain(..).map(Some).collect();
        let mut fields: Option<Vec<Option<Fields>>> = self
            .fields
            .take()
            .map(|fields| fields.into_iter().map(Some).collect());
        #[cfg(test)]
        let mut made = 0;
        let mut carried_fields = fields.as_ref().map(|_| Vec::with_capacity(cards.len()));
        let mut carried_titles = Vec::with_capacity(cards.len());
        for (card, from) in cards.iter().zip(&reused) {
            carried_titles.push(
                from.and_then(|from| titles[from].take())
                    .unwrap_or_else(|| card.display_title.to_lowercase()),
            );
            if let (Some(fields), Some(carried)) = (fields.as_mut(), carried_fields.as_mut()) {
                carried.push(
                    from.and_then(|from| fields[from].take())
                        .unwrap_or_else(|| {
                            #[cfg(test)]
                            {
                                made += 1;
                            }
                            Fields::of(card)
                        }),
                );
            }
        }
        Self {
            cards: cards_key(cards),
            titles: carried_titles,
            fields: carried_fields,
            #[cfg(test)]
            made,
            #[cfg(all(test, debug_assertions))]
            made_from: cards.iter().map(made_from).collect(),
        }
    }
}

#[cfg(all(test, debug_assertions))]
fn made_from(card: &Card) -> (String, String, Vec<String>) {
    (
        card.display_title.clone(),
        card.name.clone(),
        card.tags.clone(),
    )
}

#[derive(Debug, Default)]
pub(crate) struct Order {
    /// Indices into the cards, in shown order.
    indices: Vec<usize>,
    counts: Counts,
    sections: Vec<Section>,
    options: Options,
    /// The fields of the cards the order was built from, while they're
    /// known to match them.
    prepared: Option<Prepared>,
    /// What the order was built from. Every change goes through
    /// `App::reorder`, so in the app this always matches; tests that set
    /// the cards or search directly get an order computed on the spot.
    built_from: Inputs,
    /// Grows each time the order is rebuilt or kept for new cards.
    revision: u64,
    #[cfg(test)]
    pub(crate) rebuilds: usize,
    /// How many cards' search fields have been made.
    #[cfg(test)]
    pub(crate) prepared_cards: usize,
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
    group: GroupBy,
    collapsed: BTreeSet<SectionKey>,
}

/// Everything the shown order depends on, borrowed from the app.
#[derive(Clone, Copy)]
pub(crate) struct Of<'a> {
    pub(crate) cards: &'a [Card],
    pub(crate) search: &'a str,
    pub(crate) query: Query,
    pub(crate) filters: &'a Filters,
    pub(crate) group: GroupBy,
    /// The sections collapsed this session, which may name some not shown.
    pub(crate) collapsed: &'a BTreeSet<SectionKey>,
}

impl Of<'_> {
    fn cards_key(&self) -> (usize, usize) {
        cards_key(self.cards)
    }
}

impl Inputs {
    fn of(of: Of<'_>) -> Self {
        Self {
            cards: of.cards_key(),
            search: of.search.to_owned(),
            query: of.query,
            filters: of.filters.clone(),
            group: of.group,
            collapsed: of.collapsed.clone(),
        }
    }

    fn fit(&self, of: Of<'_>) -> bool {
        self.cards == of.cards_key()
            && self.search == of.search
            && self.query == of.query
            && self.filters == *of.filters
            && self.group == of.group
            && self.collapsed == *of.collapsed
    }
}

impl Order {
    /// The shown indices for `of`, with the empty states' counts. Hidden
    /// cards go unless Show hidden is on, then the store, platform, and tag filters
    /// and the search narrow, and what's left is sorted. Sort keys are
    /// computed once here, not per comparison. `prepared` is reused when it
    /// was made from these cards; otherwise new ones are made and returned.
    pub(crate) fn sorted(of: Of<'_>, prepared: Option<&Prepared>) -> Built {
        let Of {
            cards,
            search,
            query,
            filters,
            group,
            collapsed,
        } = of;
        let search = Search::new(search);
        let fresh = match prepared {
            Some(prepared)
                if prepared.fit(cards) && (search.is_empty() || prepared.fields.is_some()) =>
            {
                None
            }
            _ => Some(Prepared::of(cards, !search.is_empty())),
        };
        let ready = fresh
            .as_ref()
            .or(prepared)
            .expect("made fresh unless the given ones fit");
        let fields = ready.fields.as_deref();
        let mut counts = Counts::default();
        let mut indices: Vec<usize> = Vec::new();
        for (index, card) in cards.iter().enumerate() {
            counts.hidden += usize::from(card.hidden);
            // Fields are only missing when there's no search.
            let found =
                filters.admit(card) && fields.is_none_or(|fields| fields[index].matched(&search));
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
        // Only the key being sorted by needs these, so the others skip the
        // allocations.
        let sources: Vec<Option<String>> = if query.sort == SortKey::Source {
            cards
                .iter()
                .map(|card| card.source.label().map(str::to_lowercase))
                .collect()
        } else {
            Vec::new()
        };
        let tags: Vec<Vec<String>> = if query.sort == SortKey::Tags {
            cards
                .iter()
                .map(|card| {
                    let mut tags: Vec<String> =
                        card.tags.iter().map(|tag| tag.to_lowercase()).collect();
                    tags.sort_unstable();
                    tags
                })
                .collect()
        } else {
            Vec::new()
        };
        let fields: Vec<SortFields<'_>> = cards
            .iter()
            .zip(&ready.titles)
            .enumerate()
            .map(|(index, (card, title))| SortFields {
                source: sources.get(index).and_then(Option::as_deref),
                tags: tags.get(index).map_or(&[], Vec::as_slice),
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
        let sections = sections(cards, &indices, group, collapsed);
        Built {
            indices,
            counts,
            sections,
            prepared: fresh,
        }
    }

    pub(crate) fn rebuild(&mut self, of: Of<'_>) {
        self.replace(Self::sorted(of, self.prepared.as_ref()), of);
    }

    /// Takes what `sorted` built for exactly these inputs.
    pub(crate) fn replace(&mut self, built: Built, of: Of<'_>) {
        self.indices = built.indices;
        self.counts = built.counts;
        self.sections = built.sections;
        if let Some(prepared) = built.prepared {
            #[cfg(test)]
            {
                self.prepared_cards += prepared.made;
            }
            self.prepared = Some(prepared);
        }
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

    /// The cards changed where they are, which their address can't tell,
    /// so the next rebuild lists their stores, platforms, and tags again.
    pub(crate) fn cards_edited(&mut self) {
        self.built_from.cards = (0, 0);
        self.prepared = None;
    }

    /// The cards' favorite or hidden flags changed where they are. Neither
    /// is searched or offered as a filter value, so the prepared fields stay.
    pub(crate) fn flags_edited(&mut self) {
        self.built_from.cards = (0, 0);
    }

    /// Moves the prepared fields from `old`, the cards the order was built
    /// from, over to `cards`, which are about to replace them, so only the
    /// cards whose name, title, or tags changed are prepared again.
    pub(crate) fn carry(&mut self, old: &[Card], cards: &[Card]) {
        self.prepared = self
            .prepared
            .take()
            .filter(|prepared| prepared.fit(old))
            .map(|prepared| {
                let carried = prepared.carried(old, cards);
                #[cfg(test)]
                {
                    self.prepared_cards += carried.made;
                }
                carried
            });
    }

    /// The prepared fields, for `merge_cards` to hand `sorted` after `carry`.
    pub(crate) fn prepared(&self) -> Option<&Prepared> {
        self.prepared.as_ref()
    }

    /// Records that the order still fits new cards whose sort, filter, and
    /// search inputs match the old ones card for card, so their stores,
    /// platforms, and tags are the same too.
    pub(crate) fn keep(&mut self, of: Of<'_>) {
        // Fields made from the old cards fit the new ones too, so a quiet
        // rescan makes none. Any others are for cards long gone.
        let (old, new) = (self.built_from.cards, of.cards_key());
        self.prepared = self
            .prepared
            .take()
            .filter(|prepared| prepared.cards == old || prepared.cards == new)
            .map(|mut prepared| {
                prepared.cards = new;
                prepared
            });
        self.built_from = Inputs::of(of);
        self.revision = self.revision.wrapping_add(1);
    }

    /// Changes whenever the shown order or the cards in it may have, while
    /// the order fits `of`. Cards only change as a whole or through
    /// `App::reorder`, so the same revision means the same cards.
    pub(crate) fn revision(&self, of: Of<'_>) -> Option<u64> {
        self.built_from.fit(of).then_some(self.revision)
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
        Self::sorted(of, self.prepared.as_ref())
            .indices
            .into_iter()
            .map(|index| &cards[index])
            .collect()
    }

    /// The shown order's sections: one `All` while the library isn't
    /// grouped, else each with a card in it, in the order they show.
    pub(crate) fn sections(&self, of: Of<'_>) -> Cow<'_, [Section]> {
        if self.fits(of) {
            Cow::Borrowed(&self.sections)
        } else {
            Cow::Owned(Self::sorted(of, self.prepared.as_ref()).sections)
        }
    }

    pub(crate) fn counts(&self, of: Of<'_>) -> Counts {
        if self.fits(of) {
            self.counts
        } else {
            Self::sorted(of, self.prepared.as_ref()).counts
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

/// How many games Recently played holds (plan 0062, question 20).
pub(crate) const RECENT: usize = 10;

/// The sections `indices`, already in shown order, fall into under
/// `group`. A card with several tags sits in each of their sections (plan
/// 0062, question 12). Sections run A to Z, with the catch-all last, keep
/// the shown order inside, and only show with a card in them. Any grouping
/// pins Recently played first and Hidden last (question 21): a hidden game
/// sits only in Hidden, and a recent one in Recently played as well as its
/// usual sections. Group by Recent is just the pinned sections around All,
/// which holds every game that isn't hidden, as the flat library does.
fn sections(
    cards: &[Card],
    indices: &[usize],
    group: GroupBy,
    collapsed: &BTreeSet<SectionKey>,
) -> Vec<Section> {
    if group == GroupBy::None {
        return vec![Section {
            key: SectionKey::All,
            cards: indices.to_vec(),
            collapsed: false,
        }];
    }
    // Hidden games are only here with Show hidden on.
    let (hidden, indices): (Vec<usize>, Vec<usize>) =
        indices.iter().partition(|&&index| cards[index].hidden);
    let indices = indices.as_slice();
    // The newest first whatever the sort, and the shown order between
    // games played at the same moment, since the sort is stable.
    let mut recent: Vec<usize> = indices
        .iter()
        .copied()
        .filter(|&index| cards[index].last_played_at.is_some())
        .collect();
    recent.sort_by(|&a, &b| cards[b].last_played_at.cmp(&cards[a].last_played_at));
    recent.truncate(RECENT);
    let grouped: Vec<(SectionKey, Vec<usize>)> = match group {
        GroupBy::None => Vec::new(),
        // The same All as the flat library, so switching between the two
        // pairs its cards slot for slot.
        GroupBy::Recent => vec![(SectionKey::All, indices.to_vec())],
        GroupBy::Tag => {
            let mut tags: BTreeMap<(String, &str), Vec<usize>> = BTreeMap::new();
            let mut untagged = Vec::new();
            for &index in indices {
                // A tag listed twice still puts the card in its section once.
                let own: BTreeSet<&str> = cards[index].tags.iter().map(String::as_str).collect();
                if own.is_empty() {
                    untagged.push(index);
                }
                for tag in own {
                    tags.entry((tag.to_lowercase(), tag))
                        .or_default()
                        .push(index);
                }
            }
            tags.into_iter()
                .map(|((_, tag), cards)| (SectionKey::Tag(tag.to_owned()), cards))
                .chain([(SectionKey::NoTag, untagged)])
                .collect()
        }
        GroupBy::Source => {
            let mut sources: BTreeMap<(String, &'static str), Vec<usize>> = BTreeMap::new();
            let mut unknown = Vec::new();
            for &index in indices {
                match cards[index].source.label() {
                    Some(label) => sources
                        .entry((label.to_lowercase(), label))
                        .or_default()
                        .push(index),
                    None => unknown.push(index),
                }
            }
            sources
                .into_iter()
                .map(|((_, label), cards)| (SectionKey::Source(label), cards))
                .chain([(SectionKey::NoSource, unknown)])
                .collect()
        }
        GroupBy::State => {
            let mut states: [(SectionKey, Vec<usize>); 3] = [
                (SectionKey::Running, Vec::new()),
                (SectionKey::OtherState, Vec::new()),
                (SectionKey::NotInstalled, Vec::new()),
            ];
            for &index in indices {
                let key = state_section(&cards[index]);
                if let Some((_, cards)) = states.iter_mut().find(|(section, _)| *section == key) {
                    cards.push(index);
                }
            }
            states.into_iter().collect()
        }
    };
    std::iter::once((SectionKey::Recent, recent))
        .chain(grouped)
        .chain([(SectionKey::Hidden, hidden)])
        .filter(|(_, cards)| !cards.is_empty())
        .map(|(key, cards)| Section {
            collapsed: collapsed.contains(&key),
            key,
            cards,
        })
        .collect()
}

/// The section a card sits in while grouped by State: running, not
/// installed, or neither.
pub(crate) fn state_section(card: &Card) -> SectionKey {
    if card.active() {
        SectionKey::Running
    } else if card.not_installed() {
        SectionKey::NotInstalled
    } else {
        SectionKey::OtherState
    }
}

/// Whether two versions of a card would sort, filter, match a search, and
/// fall into sections alike under `group`. A State section moves with the
/// runtime and Steam's install state, which only grouping by State reads, so
/// every other rescan keeps its shortcut.
pub(crate) fn same_inputs(a: &Card, b: &Card, group: GroupBy) -> bool {
    (group != GroupBy::State || state_section(a) == state_section(b)) && same_order_inputs(a, b)
}

fn same_order_inputs(a: &Card, b: &Card) -> bool {
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
