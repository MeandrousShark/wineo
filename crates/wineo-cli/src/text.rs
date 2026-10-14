use unicode_width::UnicodeWidthStr;

/// Picks the word for `count`, like `plural(n, "entry", "entries")`.
pub fn plural<'a, T: PartialEq + From<u8>>(count: T, one: &'a str, many: &'a str) -> &'a str {
    if count == T::from(1) { one } else { many }
}

/// Like `1 error` or `2 errors`.
pub fn counted<T: PartialEq + From<u8> + std::fmt::Display + Copy>(
    count: T,
    one: &str,
    many: &str,
) -> String {
    format!("{count} {}", plural(count, one, many))
}

/// The closing line of a batch over games, like `Imported 2 of 3 games.`
pub fn games_summary(verb: &str, done: usize, total: usize) -> String {
    format!(
        "{verb} {done} of {total} {}.",
        plural(total, "game", "games")
    )
}

/// How many terminal columns `text` takes, which is what a table lines up on.
/// Bytes and chars are both wrong for accents and wide characters.
pub fn width(text: &str) -> usize {
    text.width()
}

/// The widest of `items`, or 0 for none.
pub fn widest<'a>(items: impl IntoIterator<Item = &'a str>) -> usize {
    items.into_iter().map(width).max().unwrap_or(0)
}

/// `text` followed by spaces up to `width` columns, never cut short.
pub fn pad(text: &str, width: usize) -> String {
    format!(
        "{text}{}",
        " ".repeat(width.saturating_sub(self::width(text)))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_counts_columns_not_bytes_or_chars() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width("caf\u{e9}"), 4);
        assert_eq!(width("\u{539f}\u{795e}"), 4);
    }

    #[test]
    fn a_combining_mark_adds_no_columns() {
        assert_eq!(width("e\u{301}"), 1);
        assert_eq!(width("cafe\u{301}"), width("caf\u{e9}"));
    }

    #[test]
    fn an_emoji_is_two_columns_and_a_zero_width_joiner_sequence_is_one_emoji_wide() {
        assert_eq!(width("\u{1f3ae}"), 2);
        // Man, ZWJ, woman, ZWJ, girl: unicode_width treats the joined
        // sequence as the single glyph a terminal draws.
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
        assert_eq!(width(family), 2);
    }

    #[test]
    fn pad_fills_to_the_display_width_and_never_cuts() {
        assert_eq!(pad("e\u{301}", 3), "e\u{301}  ");
        assert_eq!(pad("\u{1f3ae}", 4), "\u{1f3ae}  ");
        assert_eq!(pad("\u{539f}\u{795e}", 2), "\u{539f}\u{795e}");
    }
}
