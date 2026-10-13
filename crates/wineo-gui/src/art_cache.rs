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

/// Library art by key and whether it was decoded dimmed. A card normally
/// holds one variant; the other only lingers while the wanted one decodes.
pub(crate) struct ArtCache<K> {
    entries: HashMap<(K, bool), Entry>,
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
        self.entries.contains_key(&(key.clone(), dimmed))
    }

    /// The variant asked for, or the other one while it is still decoding,
    /// so a card switching between dimmed and not never draws blank.
    pub(crate) fn shown(&self, key: &K, dimmed: bool) -> Option<&image::Handle> {
        self.entries
            .get(&(key.clone(), dimmed))
            .or_else(|| self.entries.get(&(key.clone(), !dimmed)))
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
        let bytes = match &handle {
            image::Handle::Rgba { pixels, .. } => pixels.len(),
            _ => 0,
        };
        self.remove(&key, dimmed);
        self.clock = self.clock.wrapping_add(1);
        self.bytes += bytes;
        self.entries.insert(
            (key, dimmed),
            Entry {
                handle,
                bytes,
                used: self.clock,
            },
        );
    }

    pub(crate) fn remove(&mut self, key: &K, dimmed: bool) {
        if let Some(old) = self.entries.remove(&(key.clone(), dimmed)) {
            self.bytes -= old.bytes;
        }
    }

    /// Marks art as just used. The view can't record what it draws, so this
    /// runs for the range around the viewport each time it is computed.
    pub(crate) fn touch(&mut self, key: &K) {
        self.clock = self.clock.wrapping_add(1);
        for dimmed in [false, true] {
            if let Some(entry) = self.entries.get_mut(&(key.clone(), dimmed)) {
                entry.used = self.clock;
            }
        }
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        let mut bytes = 0;
        self.entries.retain(|(key, _), entry| {
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
    pub(crate) fn evict(&mut self, protected: impl Fn(&K) -> bool) {
        while self.bytes > self.budget {
            let Some(oldest) = self
                .entries
                .iter()
                .filter(|((key, _), _)| !protected(key))
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone())
            else {
                return;
            };
            self.remove(&oldest.0, oldest.1);
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
