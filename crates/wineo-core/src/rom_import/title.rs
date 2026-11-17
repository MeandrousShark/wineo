//! A game's title from its file name: the stem without bracketed notes.

use crate::config::TITLE_MAX_CHARS;

/// `name` without `(...)` and `[...]` notes, whitespace tidied. An opening
/// bracket that is never closed is kept as written.
fn without_notes(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    let mut at = 0;
    while at < chars.len() {
        if matches!(chars[at], '(' | '[')
            && let Some(end) = closing(&chars, at)
        {
            out.push(' ');
            at = end + 1;
            continue;
        }
        out.push(chars[at]);
        at += 1;
    }
    out
}

/// Where the bracket opened at `start` closes, counting nested ones.
fn closing(chars: &[char], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (at, char) in chars.iter().enumerate().skip(start) {
        match char {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
    }
    None
}

/// Runs of whitespace and control characters made one space, then trimmed.
fn tidy(text: &str) -> String {
    text.split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// At most `TITLE_MAX_CHARS` characters, never ending in whitespace.
fn cut(title: String) -> String {
    if title.chars().count() <= TITLE_MAX_CHARS {
        return title;
    }
    let cut: String = title.chars().take(TITLE_MAX_CHARS).collect();
    cut.trim_end().to_owned()
}

fn usable(text: &str) -> bool {
    text.chars().any(char::is_alphanumeric)
}

/// The title for a game whose file (or folder) stem is `stem`: the stem
/// without its notes, else the stem itself, else a name made from it.
pub(super) fn clean(stem: &str) -> String {
    let cleaned = tidy(&without_notes(stem));
    if usable(&cleaned) {
        return cut(cleaned);
    }
    let stem = tidy(stem);
    if !stem.is_empty() {
        return cut(stem);
    }
    crate::targets::suggest_name(&cleaned, "")
}
