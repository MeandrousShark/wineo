//! Poster titles, wrapped to at most two balanced lines.

use std::cell::RefCell;
use std::collections::HashMap;

use iced::advanced::text::{self, Paragraph as _};
use iced::{Font, Pixels, Size};
use unicode_segmentation::UnicodeSegmentation as _;

const ELLIPSIS: &str = "...";

/// The title's lines for a poster `width` wide, measured in the font its text
/// widget draws with. Shaping is too slow to repeat for every title on every
/// frame while something animates, so results are kept per title and style.
pub(crate) fn lines(title: &str, width: f32, size: f32, font: Font) -> Vec<String> {
    thread_local! {
        static CACHE: RefCell<Cache<Vec<String>>> = RefCell::new(Cache::new(CACHE_LIMIT));
    }
    let key = (title.to_owned(), width.to_bits(), size.to_bits(), font);
    CACHE.with_borrow_mut(|cache| {
        cache.get_or_insert_with(key, || {
            balance(title, width, |content| measure(content, size, font))
        })
    })
}

/// The size a text widget with default settings takes for `content` when
/// it may wrap within `width`, for predicting the height of wrapped labels.
pub(crate) fn wrapped_size(content: &str, width: f32, size: f32, font: Font) -> Size {
    #[cfg(test)]
    crate::fonts::install_once();
    thread_local! {
        static CACHE: RefCell<Cache<Size>> = RefCell::new(Cache::new(CACHE_LIMIT));
    }
    let width = width.max(0.0);
    let key = (content.to_owned(), width.to_bits(), size.to_bits(), font);
    CACHE.with_borrow_mut(|cache| {
        cache.get_or_insert_with(key, || {
            let bounds = <iced::Renderer as text::Renderer>::Paragraph::with_text(text::Text {
                content,
                bounds: Size::new(width, f32::INFINITY),
                size: Pixels(size),
                line_height: text::LineHeight::default(),
                font,
                align_x: text::Alignment::Default,
                align_y: iced::alignment::Vertical::Top,
                shaping: text::Shaping::default(),
                wrapping: text::Wrapping::default(),
            })
            .min_bounds();
            // The widget never reports more width than it was offered.
            Size::new(bounds.width.min(width), bounds.height)
        })
    })
}

/// Comfortably more titles than one screen draws, so a redraw never evicts
/// what it is about to reuse.
const CACHE_LIMIT: usize = 8192;

type Key = (String, u32, u32, Font);

/// Measurements by text and style. Renamed and removed titles stop being
/// used, so once the cache is full it drops whatever wasn't used recently.
struct Cache<V> {
    entries: HashMap<Key, (V, u64)>,
    tick: u64,
    limit: usize,
}

impl<V: Clone> Cache<V> {
    fn new(limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            tick: 0,
            limit: limit.max(2),
        }
    }

    fn get_or_insert_with(&mut self, key: Key, wrap: impl FnOnce() -> V) -> V {
        self.tick += 1;
        if let Some((lines, used)) = self.entries.get_mut(&key) {
            *used = self.tick;
            return lines.clone();
        }
        if self.entries.len() >= self.limit {
            // Keeping the newer half evicts in bulk, so filling past the
            // limit costs one sweep per limit / 2 inserts.
            let keep = (self.limit / 2) as u64;
            let tick = self.tick;
            self.entries.retain(|_, (_, used)| tick - *used < keep);
        }
        let lines = wrap();
        self.entries.insert(key, (lines.clone(), self.tick));
        lines
    }
}

/// Two lines' height at `size` with the default line height, reserved under
/// every poster so rows line up whether a title wraps or not.
pub(crate) fn two_line_height(size: f32) -> f32 {
    2.0 * text::LineHeight::default().to_absolute(Pixels(size)).0
}

/// The rendered width of one unwrapped line, shaped as a text widget with
/// default settings shapes it.
fn measure(content: &str, size: f32, font: Font) -> f32 {
    #[cfg(test)]
    crate::fonts::install_once();
    <iced::Renderer as text::Renderer>::Paragraph::with_text(text::Text {
        content,
        bounds: Size::INFINITE,
        size: Pixels(size),
        line_height: text::LineHeight::default(),
        font,
        align_x: text::Alignment::Default,
        align_y: iced::alignment::Vertical::Top,
        shaping: text::Shaping::default(),
        wrapping: text::Wrapping::None,
    })
    .min_width()
}

/// Splits `title` into one or two lines no wider than `width`. Two lines break
/// at the space that makes them closest in width, so a short word is never
/// left alone when a more even split fits. Without any fitting split, line 2
/// ends in "..." at a word boundary, and a word too long for a line is cut.
pub(crate) fn balance(title: &str, width: f32, measure: impl Fn(&str) -> f32) -> Vec<String> {
    let words: Vec<&str> = title.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let whole = words.join(" ");
    if measure(&whole) <= width {
        return vec![whole];
    }
    if words.len() == 1 {
        return vec![cut(&whole, width, &measure)];
    }
    let mut best: Option<(f32, String, String)> = None;
    for split in 1..words.len() {
        let first = words[..split].join(" ");
        let second = words[split..].join(" ");
        let (first_width, second_width) = (measure(&first), measure(&second));
        if first_width > width || second_width > width {
            continue;
        }
        let gap = (first_width - second_width).abs();
        if best.as_ref().is_none_or(|(best_gap, ..)| gap < *best_gap) {
            best = Some((gap, first, second));
        }
    }
    if let Some((_, first, second)) = best {
        return vec![first, second];
    }
    // Nothing fits on two lines, so fill line 1 and shorten line 2.
    let taken = (1..words.len())
        .rev()
        .find(|split| measure(&words[..*split].join(" ")) <= width)
        .unwrap_or(1);
    let first = words[..taken].join(" ");
    let first = if measure(&first) <= width {
        first
    } else {
        cut(&first, width, &measure)
    };
    let rest = &words[taken..];
    let second = rest.join(" ");
    if measure(&second) <= width {
        return vec![first, second];
    }
    let second = (1..rest.len())
        .rev()
        .map(|end| ellipsized(&rest[..end].join(" ")))
        .find(|line| measure(line) <= width)
        .unwrap_or_else(|| cut(rest[0], width, &measure));
    vec![first, second]
}

/// `line` cut to fit `width` with an ellipsis. Cuts fall between grapheme
/// clusters, so an accent or a joined emoji is never split from its base.
fn cut(line: &str, width: f32, measure: &impl Fn(&str) -> f32) -> String {
    line.grapheme_indices(true)
        .rev()
        .map(|(index, _)| index)
        .filter(|end| *end > 0)
        .map(|end| ellipsized(&line[..end]))
        .find(|candidate| measure(candidate) <= width)
        .unwrap_or_else(|| ELLIPSIS.to_owned())
}

/// Punctuation that ended a cut clause reads oddly before the ellipsis.
fn ellipsized(line: &str) -> String {
    let line = line.trim_end_matches([' ', ',', ':', ';', '-']);
    format!("{line}{ELLIPSIS}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seven pixels a character keeps expected splits easy to reason about.
    fn chars(line: &str) -> f32 {
        line.chars().count() as f32 * 7.0
    }

    #[test]
    fn a_short_title_stays_on_one_line() {
        assert_eq!(balance("Portal 2", 120.0, chars), ["Portal 2"]);
        assert_eq!(balance("  Portal   2 ", 120.0, chars), ["Portal 2"]);
        assert!(balance("   ", 120.0, chars).is_empty());
    }

    #[test]
    fn balanced_breaks_keep_short_words_off_their_own_line() {
        assert_eq!(
            balance("Sid Meier's Civilization VI", 120.0, chars),
            ["Sid Meier's", "Civilization VI"]
        );
        assert_eq!(
            balance("Grand Theft Auto IV: The Complete Edition", 140.0, chars),
            ["Grand Theft Auto IV:", "The Complete Edition"]
        );
    }

    #[test]
    fn existing_punctuation_stays_with_its_word() {
        assert_eq!(
            balance("Half-Life 2: Episode One", 112.0, chars),
            ["Half-Life 2:", "Episode One"]
        );
        assert_eq!(
            balance("Hello, World! (Deluxe)", 112.0, chars),
            ["Hello, World!", "(Deluxe)"]
        );
    }

    #[test]
    fn a_title_too_long_for_two_lines_ends_in_an_ellipsis() {
        assert_eq!(
            balance(
                "The Elder Scrolls V: Skyrim Special Edition Anniversary",
                140.0,
                chars
            ),
            ["The Elder Scrolls V:", "Skyrim Special..."]
        );
        // The cut clause's colon gives way to the ellipsis.
        assert_eq!(
            balance("Alpha Beta Gamma: Delta Epsilon Zeta", 84.0, chars),
            ["Alpha Beta", "Gamma..."]
        );
    }

    #[test]
    fn an_overlong_word_is_cut_with_an_ellipsis() {
        assert_eq!(
            balance("Supercalifragilistic", 84.0, chars),
            ["Supercali..."]
        );
        assert_eq!(
            balance("Supercalifragilistic Adventures", 84.0, chars),
            ["Supercali...", "Adventures"]
        );
        assert_eq!(
            balance("Go Supercalifragilistic", 84.0, chars),
            ["Go", "Supercali..."]
        );
        assert_eq!(balance("Wide", 7.0, chars), ["..."]);
    }

    #[test]
    fn a_long_kebab_name_after_a_fitting_first_line_is_cut_on_line_two() {
        assert_eq!(
            balance(
                "Already migrated as disco-elysium-the-final-cut.",
                140.0,
                chars
            ),
            ["Already migrated as", "disco-elysium-the..."]
        );
    }

    #[test]
    fn the_cache_stays_bounded_and_keeps_titles_in_use() {
        let key = |title: &str| (title.to_owned(), 0, 0, Font::DEFAULT);
        let mut cache = Cache::new(8);
        for index in 0..1000 {
            let title = format!("Game {index}");
            let _ = cache.get_or_insert_with(key(&title), || vec![title.clone()]);
            // A title on screen is looked up on every redraw.
            let _ = cache.get_or_insert_with(key("Shown"), || vec!["Shown".to_owned()]);
            assert!(cache.entries.len() <= 8);
        }
        let mut wrapped = false;
        let _ = cache.get_or_insert_with(key("Shown"), || {
            wrapped = true;
            Vec::new()
        });
        assert!(!wrapped);
    }

    #[test]
    fn cuts_keep_combining_marks_and_joined_emoji_whole() {
        // Decomposed é is two chars, so a char cut could drop its accent.
        let accented = "e\u{301}".repeat(10);
        assert_eq!(balance(&accented, 42.0, chars), ["e\u{301}..."]);
        // Woman technologist is three chars joined by a zero width joiner.
        let coder = "\u{1f469}\u{200d}\u{1f4bb}";
        assert_eq!(
            balance(&coder.repeat(3), 49.0, chars),
            [format!("{coder}...")]
        );
    }

    #[test]
    fn real_measurement_wraps_the_maintainer_examples() {
        let size = 13.0;
        let width = 120.0;
        for title in [
            "Sid Meier's Civilization VI",
            "Grand Theft Auto IV: The Complete Edition",
        ] {
            let lines = balance(title, width, |line| measure(line, size, Font::DEFAULT));
            assert!(lines.len() <= 2, "{lines:?}");
            for line in &lines {
                assert!(
                    measure(line, size, Font::DEFAULT) <= width,
                    "{line:?} is too wide"
                );
            }
        }
        assert!(measure("Wide", size, Font::DEFAULT) > measure("W", size, Font::DEFAULT));
    }
}
