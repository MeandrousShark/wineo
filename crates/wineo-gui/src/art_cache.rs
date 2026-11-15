use std::collections::HashMap;
use std::hash::Hash;

use iced::widget::image;

/// Decoded library art kept for cards that left the screen, so scrolling
/// back draws at once. A poster decodes to 320x480 RGBA, 600 KiB. A
/// 2560x1440 window fits 13 columns and shows parts of up to 6 rows of
/// cards about 320 px tall: 78 posters, 46 MiB a screen. 96 MiB holds two
/// such screens, more than the 10 rows `load_visible_art` keeps decoded
/// around the viewport, while a 300 game library decodes to about 180 MiB.
pub(crate) const BUDGET: usize = 96 * 1024 * 1024;

struct Entry {
    handle: image::Handle,
    bytes: usize,
    used: u64,
}

/// Which way coverflow's warp leans a side cover, kept apart in the cache
/// from the flat art it's made from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) enum Tilt {
    #[default]
    None,
    Left,
    Right,
}

impl Tilt {
    const ALL: [Self; 3] = [Self::None, Self::Left, Self::Right];
    pub(crate) const SIDES: [Self; 2] = [Self::Left, Self::Right];
}

/// Library art by key, whether it was decoded gray, and its tilt. A card
/// normally holds one gray or color variant; the other only lingers while
/// the wanted one decodes. A tilt is coverflow's warp of the flat art its
/// key names.
pub(crate) struct ArtCache<K> {
    entries: HashMap<(K, bool, Tilt), Entry>,
    bytes: usize,
    clock: u64,
    budget: usize,
}

impl<K> Default for ArtCache<K> {
    fn default() -> Self {
        Self::with_budget(BUDGET)
    }
}

impl<K> ArtCache<K> {
    pub(crate) fn with_budget(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            clock: 0,
            budget,
        }
    }
}

impl<K: Clone + Eq + Hash> ArtCache<K> {
    pub(crate) fn contains(&self, key: &K, dimmed: bool) -> bool {
        self.entries
            .contains_key(&(key.clone(), dimmed, Tilt::None))
    }

    /// Every key held, once per variant.
    pub(crate) fn keys(&self) -> impl Iterator<Item = &K> {
        self.entries.keys().map(|(key, _, _)| key)
    }

    /// The variant asked for, or the other one while it is still decoding,
    /// so a card switching between dimmed and not never draws blank.
    pub(crate) fn shown(&self, key: &K, dimmed: bool) -> Option<&image::Handle> {
        self.entries
            .get(&(key.clone(), dimmed, Tilt::None))
            .or_else(|| self.entries.get(&(key.clone(), !dimmed, Tilt::None)))
            .map(|entry| &entry.handle)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    pub(crate) fn insert(&mut self, key: K, dimmed: bool, handle: image::Handle) {
        self.insert_tilted(key, dimmed, Tilt::None, handle);
    }

    pub(crate) fn insert_tilted(
        &mut self,
        key: K,
        dimmed: bool,
        tilt: Tilt,
        handle: image::Handle,
    ) {
        let bytes = match &handle {
            image::Handle::Rgba { pixels, .. } => pixels.len(),
            _ => 0,
        };
        self.remove_tilted(&key, dimmed, tilt);
        self.clock = self.clock.wrapping_add(1);
        self.bytes += bytes;
        self.entries.insert(
            (key, dimmed, tilt),
            Entry {
                handle,
                bytes,
                used: self.clock,
            },
        );
    }

    pub(crate) fn contains_tilted(&self, key: &K, dimmed: bool, tilt: Tilt) -> bool {
        self.entries.contains_key(&(key.clone(), dimmed, tilt))
    }

    /// The tilt asked for, gray or not as asked, or the other while that
    /// one is still warping.
    pub(crate) fn shown_tilted(&self, key: &K, dimmed: bool, tilt: Tilt) -> Option<&image::Handle> {
        self.entries
            .get(&(key.clone(), dimmed, tilt))
            .or_else(|| self.entries.get(&(key.clone(), !dimmed, tilt)))
            .map(|entry| &entry.handle)
    }

    pub(crate) fn remove(&mut self, key: &K, dimmed: bool) {
        self.remove_tilted(key, dimmed, Tilt::None);
    }

    fn remove_tilted(&mut self, key: &K, dimmed: bool, tilt: Tilt) {
        if let Some(old) = self.entries.remove(&(key.clone(), dimmed, tilt)) {
            self.bytes -= old.bytes;
        }
    }

    /// Marks art as just used. The view can't record what it draws, so this
    /// runs for the range around the viewport each time it is computed.
    pub(crate) fn touch(&mut self, key: &K) {
        self.clock = self.clock.wrapping_add(1);
        for dimmed in [false, true] {
            for tilt in Tilt::ALL {
                if let Some(entry) = self.entries.get_mut(&(key.clone(), dimmed, tilt)) {
                    entry.used = self.clock;
                }
            }
        }
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        let mut bytes = 0;
        self.entries.retain(|(key, _, _), entry| {
            let kept = keep(key);
            if kept {
                bytes += entry.bytes;
            }
            kept
        });
        self.bytes = bytes;
    }

    /// Drops the least recently used art until the cache fits its budget,
    /// never anything in `protected`. Protected art alone can exceed the
    /// budget on a very large window, and then the cache stays over.
    #[cfg(test)]
    pub(crate) fn evict(&mut self, protected: impl Fn(&K) -> bool) {
        self.evict_where(|key, _| protected(key));
    }

    /// `evict`, protecting by key and tilt, so a cover's flat art can stay
    /// while a tilt it no longer needs goes.
    pub(crate) fn evict_where(&mut self, protected: impl Fn(&K, Tilt) -> bool) {
        while self.bytes > self.budget {
            let Some(oldest) = self
                .entries
                .iter()
                .filter(|((key, _, tilt), _)| !protected(key, *tilt))
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone())
            else {
                return;
            };
            if let Some(old) = self.entries.remove(&oldest) {
                self.bytes -= old.bytes;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(bytes: usize) -> image::Handle {
        image::Handle::from_rgba(1, (bytes / 4) as u32, vec![0; bytes])
    }

    #[test]
    fn evicts_least_recently_used_art_outside_the_protected_keys() {
        let mut cache = ArtCache::with_budget(12);
        cache.insert("a", false, handle(4));
        cache.insert("b", false, handle(4));
        cache.insert("c", true, handle(4));
        cache.touch(&"a");
        cache.insert("d", false, handle(4));
        cache.evict(|key| *key == "b");
        // "c" is the oldest that isn't protected; "b" is older but on screen.
        assert!(!cache.contains(&"c", true));
        assert!(cache.contains(&"a", false));
        assert!(cache.contains(&"b", false));
        assert!(cache.contains(&"d", false));
        assert_eq!(cache.bytes(), 12);
    }

    #[test]
    fn protected_art_may_exceed_the_budget() {
        let mut cache = ArtCache::with_budget(4);
        cache.insert("a", false, handle(4));
        cache.insert("b", false, handle(4));
        cache.insert("c", false, handle(4));
        cache.evict(|key| *key != "c");
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.bytes(), 8);
        cache.evict(|_| true);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn tilts_are_kept_apart_from_flat_art_and_evicted_by_their_own_protection() {
        let mut cache = ArtCache::with_budget(12);
        let (flat, left) = (handle(4), handle(4));
        cache.insert("a", false, flat.clone());
        cache.insert_tilted("a", false, Tilt::Left, left.clone());
        assert_eq!(cache.shown(&"a", false), Some(&flat));
        assert_eq!(cache.shown_tilted(&"a", false, Tilt::Left), Some(&left));
        assert_eq!(cache.shown_tilted(&"a", true, Tilt::Left), Some(&left));
        assert_eq!(cache.shown_tilted(&"a", false, Tilt::Right), None);
        assert!(cache.contains_tilted(&"a", false, Tilt::Left));
        assert!(!cache.contains_tilted(&"a", true, Tilt::Left));
        cache.insert_tilted("a", false, Tilt::Right, handle(8));
        // Over budget: the flat art is protected, the tilts by tilt.
        cache.evict_where(|_, tilt| tilt != Tilt::Left);
        assert!(!cache.contains_tilted(&"a", false, Tilt::Left));
        assert!(cache.contains(&"a", false));
        assert!(cache.contains_tilted(&"a", false, Tilt::Right));
        assert_eq!(cache.bytes(), 12);
    }

    #[test]
    fn shows_the_other_variant_until_the_wanted_one_arrives() {
        let mut cache = ArtCache::default();
        let color = handle(4);
        cache.insert("a", false, color.clone());
        assert_eq!(cache.shown(&"a", true), Some(&color));
        let gray = handle(8);
        cache.insert("a", true, gray.clone());
        assert_eq!(cache.shown(&"a", true), Some(&gray));
        assert_eq!(cache.shown(&"a", false), Some(&color));
        cache.remove(&"a", false);
        assert_eq!(cache.bytes(), 8);
        cache.retain(|_| false);
        assert_eq!(cache.bytes(), 0);
        assert_eq!(cache.shown(&"a", true), None);
    }
}
