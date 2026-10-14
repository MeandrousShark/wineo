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
