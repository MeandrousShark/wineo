//! Loading, decoding, and evicting the library's card art.

use std::collections::HashSet;

use iced::Task;
use iced::widget::image;

use crate::{App, ArtKey, GAP, Message, artwork, grid, grid_motion};

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
        let filtered = self.filtered();
        let heights: Vec<f32> = filtered
            .iter()
            .map(|card| grid::card_height(card))
            .collect();
        // Two rows either side start decoding before a card scrolls in.
        let margin = 2.0 * (heights.iter().copied().fold(0.0, f32::max) + GAP);
        let top = self.scroll_offset - margin;
        let bottom = self.scroll_offset + self.viewport_height + margin;
        let count = filtered.len();
        let mut ranges: Vec<_> = [false, true]
            .into_iter()
            .map(|open| {
                let columns = grid_motion::columns_for_viewport(self.viewport_width, open);
                let range = grid_motion::cards_between(&heights, columns, top, bottom);
                // The margin reaches above the first row, so an empty range
                // means the viewport is past this layout's last row.
                if range.is_empty() {
                    count..count
                } else {
                    range
                }
            })
            .collect();
        // A card moving between its two slots can cross the viewport while
        // neither slot is in it, but only cards between the two ranges move
        // through it, or from the other range to the end of the list when
        // one layout ends above the viewport. Both layouts are known when the
        // motion starts, so this holds for the whole motion; the tick that
        // settles it narrows again.
        if self.grid_motion.active()
            && let (Some(start), Some(end)) = (
                ranges.iter().map(|range| range.start).min(),
                ranges.iter().map(|range| range.end).max(),
            )
        {
            ranges = std::iter::once(start..end).collect();
        }
        let mut seen = HashSet::new();
        let mut keys = Vec::new();
        for range in ranges {
            for card in &filtered[range] {
                if let Some(art) = &card.art {
                    let key = (art.clone(), artwork::Kind::Grid);
                    if seen.insert(key.clone()) {
                        keys.push((key, card.not_installed()));
                    }
                }
            }
        }
        for (key, _) in &keys {
            self.images.touch(key);
        }
        self.visible_art = seen;
        self.evict_art();
        self.load_images(keys)
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
