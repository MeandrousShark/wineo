//! The library search: a card's title, name, or tag matches a search by
//! plain substring as before, or by a few strict extra rules, so `gta` finds
//! Grand Theft Auto IV and `civ 6` finds Civilization VI. Every rule is a yes
//! or no; nothing is ranked.

use std::ops::Range;

use crate::name;

/// One searchable piece of text, cut into words once so each search only
/// scans prepared strings.
#[derive(Debug, Clone)]
pub struct Field {
    lower: String,
    folded: String,
    joined: String,
    words: Vec<Word>,
}

#[derive(Debug, Clone)]
struct Word {
    /// Byte range into `joined`. Consecutive words are adjacent there, so a
    /// run of words joined together is just a longer slice.
    range: Range<usize>,
    numeral: bool,
    value: Option<u32>,
}

/// A prepared search, parsed once per change.
#[derive(Debug, Clone)]
pub struct Search {
    lower: String,
    folded: String,
    joined: String,
    words: Vec<SearchWord>,
    initials: Option<Initials>,
}

#[derive(Debug, Clone)]
struct SearchWord {
    range: Range<usize>,
    value: Option<u32>,
}

#[derive(Debug, Clone)]
struct Initials {
    first: String,
    numerals: Vec<Numeral>,
}

#[derive(Debug, Clone)]
struct Numeral {
    text: String,
    value: Option<u32>,
}

/// Apostrophes vanish instead of splitting a word, and combining accents
/// ride along with the letter before them, as in `name::from_title`.
fn dropped(character: char) -> bool {
    matches!(character, '\'' | '\u{2019}' | '\u{300}'..='\u{36f}')
}

/// Appends the folded form of one already lowercased character.
fn push_folded(character: char, folded: &mut String, joined: &mut String) {
    if dropped(character) {
        return;
    }
    match name::fold(character) {
        Some(piece) => {
            folded.push_str(piece);
            joined.push_str(piece);
        }
        None => {
            folded.push(character);
            if character.is_alphanumeric() {
                joined.push(character);
            }
        }
    }
}

const ROMAN: [&str; 30] = [
    "i", "ii", "iii", "iv", "v", "vi", "vii", "viii", "ix", "x", "xi", "xii", "xiii", "xiv", "xv",
    "xvi", "xvii", "xviii", "xix", "xx", "xxi", "xxii", "xxiii", "xxiv", "xxv", "xxvi", "xxvii",
    "xxviii", "xxix", "xxx",
];

/// The value of a canonical Roman numeral from I to XXX, given lowercase.
fn roman_value(text: &str) -> Option<u32> {
    ROMAN
        .iter()
        .position(|&numeral| numeral == text)
        .and_then(|index| u32::try_from(index + 1).ok())
}

fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// The length of `value` written in decimal when `rest` starts with it.
fn starts_with_value(rest: &str, value: u32) -> Option<usize> {
    let mut buffer = [0u8; 10];
    let mut at = buffer.len();
    let mut left = value;
    loop {
        at -= 1;
        buffer[at] = b'0' + (left % 10) as u8;
        left /= 10;
        if left == 0 {
            break;
        }
    }
    let digits = &buffer[at..];
    rest.as_bytes().starts_with(digits).then_some(digits.len())
}

impl Field {
    pub fn new(text: &str) -> Self {
        // Lowercasing the whole string, not each character, keeps a final
        // sigma as today's search sees it.
        let lower = text.to_lowercase();
        let mut folded = String::with_capacity(lower.len());
        let mut joined = String::with_capacity(lower.len());
        let mut words = Vec::new();
        let mut lowered = lower.chars();
        // The open word: its start in `joined`, and whether every character
        // so far is an ASCII digit or an uppercase Roman letter. Numerals and
        // the case and digit boundaries need the original case, so words are
        // cut here and kept as ranges into the lowered and folded form.
        let mut open: Option<(usize, bool, bool)> = None;
        // The last word character, for the case and digit boundaries.
        let mut previous: Option<char> = None;
        let close =
            |open: &mut Option<(usize, bool, bool)>, joined: &str, words: &mut Vec<Word>| {
                if let Some((start, digits, roman)) = open.take() {
                    let range = start..joined.len();
                    let piece = &joined[range.clone()];
                    if piece.is_empty() {
                        return;
                    }
                    let (numeral, value) = if digits {
                        (true, piece.parse().ok())
                    } else if roman {
                        let value = roman_value(piece);
                        (value.is_some(), value)
                    } else {
                        (false, None)
                    };
                    words.push(Word {
                        range,
                        numeral,
                        value,
                    });
                }
            };
        for character in text.chars() {
            // `str::to_lowercase` maps each character to as many characters
            // as `char::to_lowercase` does, final sigma included, so this
            // walks both strings in step.
            let count = character.to_lowercase().count();
            if dropped(character) {
                // A combining mark makes the letter before it something
                // else, so `I\u{301}` is no more a numeral than `Í` is.
                if !matches!(character, '\'' | '\u{2019}')
                    && let Some((_, digits, roman)) = open.as_mut()
                {
                    *digits = false;
                    *roman = false;
                }
                for lowered_character in lowered.by_ref().take(count) {
                    push_folded(lowered_character, &mut folded, &mut joined);
                }
                continue;
            }
            if character.is_alphanumeric() {
                let digit = character.is_numeric();
                let split = previous.is_some_and(|previous| {
                    (previous.is_lowercase() && character.is_uppercase())
                        || previous.is_numeric() != digit
                });
                if split || open.is_none() {
                    close(&mut open, &joined, &mut words);
                    open = Some((joined.len(), true, true));
                }
                if let Some((_, digits, roman)) = open.as_mut() {
                    *digits &= character.is_ascii_digit();
                    *roman &= matches!(character, 'I' | 'V' | 'X');
                }
                previous = Some(character);
            } else {
                close(&mut open, &joined, &mut words);
                previous = None;
            }
            for lowered_character in lowered.by_ref().take(count) {
                push_folded(lowered_character, &mut folded, &mut joined);
            }
        }
        close(&mut open, &joined, &mut words);
        Self {
            lower,
            folded,
            joined,
            words,
        }
    }

    fn word(&self, index: usize) -> &str {
        &self.joined[self.words[index].range.clone()]
    }

    /// Whether `rest` can be spelled by words from `index` on, each giving
    /// its first letter or, for a numeral, its whole text or its value, and
    /// the numerals after the spelling follow it word for word.
    fn spells(&self, index: usize, rest: &str, numerals: &[Numeral]) -> bool {
        if rest.is_empty() {
            return self.numerals_follow(index, numerals);
        }
        let Some(word) = self.words.get(index) else {
            return false;
        };
        let text = self.word(index);
        if let Some(first) = text.chars().next()
            && rest.starts_with(first)
            && self.spells(index + 1, &rest[first.len_utf8()..], numerals)
        {
            return true;
        }
        if !word.numeral {
            return false;
        }
        if text.len() > 1
            && rest.starts_with(text)
            && self.spells(index + 1, &rest[text.len()..], numerals)
        {
            return true;
        }
        word.value
            .and_then(|value| starts_with_value(rest, value))
            .is_some_and(|length| self.spells(index + 1, &rest[length..], numerals))
    }

    fn numerals_follow(&self, index: usize, numerals: &[Numeral]) -> bool {
        numerals.iter().enumerate().all(|(offset, numeral)| {
            self.words.get(index + offset).is_some_and(|word| {
                word.numeral
                    && ((numeral.value.is_some() && word.value == numeral.value)
                        || self.word(index + offset) == numeral.text)
            })
        })
    }
}

impl Search {
    pub fn new(search: &str) -> Self {
        let lower = search.trim().to_lowercase();
        let mut folded = String::with_capacity(lower.len());
        let mut joined = String::with_capacity(lower.len());
        for character in lower.chars() {
            push_folded(character, &mut folded, &mut joined);
        }
        let pieces: Vec<&str> = folded
            .split(|character: char| !character.is_alphanumeric())
            .filter(|piece| !piece.is_empty())
            .collect();
        // A lone `v` is just a letter, but `gta v` means 5.
        let several = pieces.len() > 1;
        let value_of = |text: &str| -> (bool, Option<u32>) {
            if is_digits(text) {
                (true, text.parse().ok())
            } else if several {
                let value = roman_value(text);
                (value.is_some(), value)
            } else {
                (false, None)
            }
        };
        let mut words = Vec::with_capacity(pieces.len());
        let mut start = 0;
        for piece in &pieces {
            words.push(SearchWord {
                range: start..start + piece.len(),
                value: value_of(piece).1,
            });
            start += piece.len();
        }
        let initials = if joined.chars().count() >= 2 {
            let mut groups = folded.split(char::is_whitespace).filter_map(|group| {
                let group: String = group.chars().filter(|c| c.is_alphanumeric()).collect();
                (!group.is_empty()).then_some(group)
            });
            groups.next().and_then(|first| {
                let numerals: Option<Vec<Numeral>> = groups
                    .map(|text| {
                        let (numeral, value) = value_of(&text);
                        numeral.then_some(Numeral { text, value })
                    })
                    .collect();
                numerals.map(|numerals| Initials { first, numerals })
            })
        } else {
            None
        };
        Self {
            lower,
            folded,
            joined,
            words,
            initials,
        }
    }

    /// An empty or all-space search matches everything.
    pub fn is_empty(&self) -> bool {
        self.lower.is_empty()
    }

    pub fn matches(&self, field: &Field) -> bool {
        if self.is_empty() {
            return true;
        }
        if field.lower.contains(&self.lower)
            || (!self.folded.is_empty() && field.folded.contains(&self.folded))
        {
            return true;
        }
        // Only punctuation or spaces: today's substring alone.
        if self.words.is_empty() {
            return false;
        }
        field.joined.contains(&self.joined) || self.word_starts(field) || self.initials(field)
    }

    fn word_starts(&self, field: &Field) -> bool {
        self.words.iter().all(|word| {
            let text = &self.joined[word.range.clone()];
            field
                .words
                .iter()
                .any(|field_word| field.joined[field_word.range.start..].starts_with(text))
                || word.value.is_some_and(|value| {
                    field
                        .words
                        .iter()
                        .any(|field_word| field_word.numeral && field_word.value == Some(value))
                })
        })
    }

    fn initials(&self, field: &Field) -> bool {
        let Some(initials) = &self.initials else {
            return false;
        };
        (0..field.words.len()).any(|start| field.spells(start, &initials.first, &initials.numerals))
    }
}

#[cfg(test)]
mod tests;
