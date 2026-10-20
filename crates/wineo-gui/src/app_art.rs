//! Loading, decoding, and evicting the library's card art.

use std::collections::HashSet;
use std::ops::ControlFlow;

use iced::widget::image;
use iced::{Rectangle, Task};

use crate::{App, ArtKey, Early, GAP, Message, artwork};

impl App {
    /// Whether any card draws `key` in color, and whether any draws it dimmed.
    pub(crate) fn art_wanted(&self, key: &ArtKey) -> (bool, bool) {
        let (art, kind) = key;
        let mut wanted = (false, false);
        for card in &self.cards {
            let uses = match kind {
                artwork::Kind::Grid => card.art.as_ref() == Some(art),
                artwork::Kind::Hero => card
                    .panel_art
                    .values()
                    .any(|paths| paths.hero.as_ref() == Some(art)),
                artwork::Kind::Logo => card
                    .panel_art
                    .values()
                    .any(|paths| paths.logo.as_ref() == Some(art)),
            };
            if uses {
                if card.not_installed() {
                    wanted.1 = true;
                } else {
                    wanted.0 = true;
                }
            }
        }
        wanted
    }
    /// Stores decoded art. Once a card's current variant arrives the other
    /// one goes, so a dimmed card holds only its grayscale copy.
    pub(crate) fn art_loaded(&mut self, key: ArtKey, dimmed: bool, handle: image::Handle) {
        let (color, gray) = self.art_wanted(&key);
        let (wanted, other_wanted) = if dimmed { (gray, color) } else { (color, gray) };
        if wanted {
            self.images.insert(key.clone(), dimmed, handle);
            if !other_wanted {
                self.images.remove(&key, !dimmed);
            }
        } else if !self.images.contains(&key, !dimmed) {
            // The card switched while this decoded; it still beats drawing
            // nothing until the variant it wants now arrives.
            self.images.insert(key, dimmed, handle);
        }
        self.evict_art();
    }
    pub(crate) fn evict_art(&mut self) {
        let panel: HashSet<ArtKey> = self
            .panel_art_keys()
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        let visible = &self.visible_art;
        self.images
            .evict(|key| visible.contains(key) || panel.contains(key));
    }

    pub(crate) fn load_visible_art(&mut self) -> Task<Message> {
        let keys = self.grid_art();
        for (key, _) in &keys {
            self.images.touch(key);
        }
        self.visible_art = keys.iter().map(|(key, _)| key.clone()).collect();
        self.evict_art();
        self.load_images(keys)
    }

    /// The art the grid draws, in the order it's needed: cards drawn in
    /// view, then moving cards headed into view or crossing it on the way,
    /// then leaving cards still in view as they fade. Each key comes once,
    /// with whether it's drawn dimmed.
    pub(crate) fn grid_art(&self) -> Vec<(ArtKey, bool)> {
        let places = self.grid_places();
        let shown = self.grid_motion.shown(&places);
        // Two rows either side start decoding before a card scrolls in.
        let tallest = places
            .slots
            .iter()
            .map(|slot| slot.height)
            .fold(0.0, f32::max);
        let margin = 2.0 * (tallest + GAP);
        let top = self.scroll_offset - margin;
        let bottom = self.scroll_offset + self.viewport_height + margin;
        let near = |bounds: Rectangle| bounds.y <= bottom && bounds.y + bounds.height >= top;
        let filtered = self.filtered();
        let in_view = (0..shown.len()).filter(|&index| near(shown[index]));
        // A card's path stays within the box its two ends span, give or
        // take the few pixels of overshoot, so one crossing the view while
        // neither end is in it still counts.
        let headed = (0..shown.len()).filter(|&index| {
            let (now, slot) = (shown[index], places.slots[index]);
            now != slot && !near(now) && near(now.union(&slot))
        });
        let leaving = self
            .grid_motion
            .leaving()
            .iter()
            .filter(|card| near(card.bounds))
            .filter_map(|leaving| self.cards.iter().find(|card| card.name == leaving.name));
        let mut seen = HashSet::new();
        let mut keys = Vec::new();
        for card in in_view
            .chain(headed)
            .map(|index| filtered[index])
            .chain(leaving)
        {
            if let Some(art) = &card.art {
                let key = (art.clone(), artwork::Kind::Grid);
                if seen.insert(key.clone()) {
                    keys.push((key, card.not_installed()));
                }
            }
        }
        keys
    }
    pub(crate) fn load_images(&mut self, keys: Vec<(ArtKey, bool)>) -> Task<Message> {
        let mut tasks = Vec::new();
        let cache = self.paths.art_cache_dir();
        for (key, dimmed) in keys {
            if self.images.contains(&key, dimmed)
                || self.failed_images.contains(&key)
                || !self.loading.insert((key.clone(), dimmed))
            {
                continue;
            }
            let decodes = self.decodes.clone();
            let cache = cache.clone();
            tasks.push(Task::perform(
                async move {
                    let decoded = match decodes.acquire_owned().await {
                        Ok(permit) => tokio::task::spawn_blocking({
                            let key = key.clone();
                            move || {
                                let decoded = decode_art(&key, dimmed, &cache);
                                drop(permit);
                                decoded
                            }
                        })
                        .await
                        .map_err(|e| e.to_string()),
                        Err(error) => Err(error.to_string()),
                    };
                    (key, decoded)
                },
                move |(key, decoded)| Message::ArtLoaded(key, dimmed, decoded.unwrap_or_else(Err)),
            ));
        }
        Task::batch(tasks)
    }

    pub(crate) fn art_result(
        &mut self,
        key: ArtKey,
        dimmed: bool,
        result: Result<image::Handle, String>,
    ) -> Early {
        self.loading.remove(&(key.clone(), dimmed));
        if self.current_art.get(&key.0.path) != Some(&key.0.identity) {
            return ControlFlow::Break(Task::none());
        }
        match result {
            Ok(handle) => self.art_loaded(key, dimmed, handle),
            Err(_) => {
                self.failed_images.insert(key);
            }
        }
        ControlFlow::Continue(Task::none())
    }
}

/// Library art as drawn, grayscale when dimmed. Runs off the UI thread.
pub(crate) fn decode_art(
    key: &ArtKey,
    dimmed: bool,
    cache: &std::path::Path,
) -> Result<image::Handle, String> {
    let handle = artwork::decode(&key.0.path, key.1, Some(cache)).map_err(|e| e.to_string())?;
    Ok(if dimmed {
        artwork::grayscale(&handle).unwrap_or(handle)
    } else {
        handle
    })
}
