//! A reader for Valve's KeyValues text, which `.vdf` and `.acf` files use. It
//! keeps file order and repeated keys, and it finds keys without regard to
//! case, since Steam's own files mix it.

use std::fmt;

/// Steam's files nest a few levels at most. The cap keeps a hostile file from
/// exhausting the stack.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Text(String),
    Object(Object),
}

impl Value {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(text) => Some(text),
            Value::Object(_) => None,
        }
    }

    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Value::Object(object) => Some(object),
            Value::Text(_) => None,
        }
    }
}

/// The entries of one `{ }` block, or of a whole file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Object {
    entries: Vec<(String, Value)>,
}

impl Object {
    /// Every entry in file order. Keys can repeat.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value))
    }

    /// The first value under `key`, ignoring case.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value)
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_text)
    }

    pub fn object(&self, key: &str) -> Option<&Object> {
        self.get(key).and_then(Value::as_object)
    }

    /// Follows `keys` through nested blocks, each one ignoring case.
    pub fn path(&self, keys: &[&str]) -> Option<&Value> {
        let (first, rest) = keys.split_first()?;
        let mut value = self.get(first)?;
        for key in rest {
            value = value.as_object()?.get(key)?;
        }
        Some(value)
    }
}

/// Where a file went wrong. Line and column are 1-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "line {}, column {}: {}",
            self.line, self.column, self.message
        )
    }
}

impl std::error::Error for ParseError {}

/// Parses a whole file. Entries whose conditional tags are false on Linux are
/// left out, so a Windows-only value never shadows the Linux one.
pub fn parse(text: &str) -> Result<Object, ParseError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    Parser { text, pos: 0 }.object(0, None)
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
}

impl Parser<'_> {
    /// Parses entries until the end of input, or until the `}` closing the
    /// block that opened at `open`. `depth` is the block's own depth.
    fn object(&mut self, depth: usize, open: Option<usize>) -> Result<Object, ParseError> {
        let mut object = Object::default();
        loop {
            self.skip_space();
            // A tag with no entry before it has nothing to apply to.
            while self.peek() == Some('[') {
                self.tag()?;
                self.skip_space();
            }
            match self.peek() {
                None => {
                    return match open {
                        Some(open) => Err(self.error_at(open, "this `{` is never closed")),
                        None => Ok(object),
                    };
                }
                Some('}') if open.is_some() => {
                    self.bump();
                    return Ok(object);
                }
                Some('}') => return Err(self.error("unexpected `}`")),
                Some(_) => {}
            }

            let key = self.key()?;
            let mut conditions = self.tags()?;
            let value = match self.peek() {
                Some('{') => {
                    if depth == MAX_DEPTH {
                        return Err(
                            self.error(format!("blocks nested deeper than {MAX_DEPTH} levels"))
                        );
                    }
                    let brace = self.pos;
                    self.bump();
                    Value::Object(self.object(depth + 1, Some(brace))?)
                }
                None | Some('}') => return Err(self.error(format!("`{key}` has no value"))),
                Some(_) => Value::Text(self.text_token()?),
            };
            conditions.extend(self.tags()?);
            // An entry that doesn't hold on Linux is left out, as if the file didn't have it.
            if conditions.iter().all(|condition| holds_on_linux(condition)) {
                object.entries.push((key, value));
            }
        }
    }

    fn key(&mut self) -> Result<String, ParseError> {
        match self.peek() {
            Some('"') => self.quoted(),
            Some('{') => Err(self.error("expected a key before `{`")),
            _ => self.unquoted(),
        }
    }

    fn text_token(&mut self) -> Result<String, ParseError> {
        match self.peek() {
            Some('"') => self.quoted(),
            _ => self.unquoted(),
        }
    }

    fn quoted(&mut self) -> Result<String, ParseError> {
        let start = self.pos;
        self.bump();
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err(self.error_at(start, "this text has no closing `\"`")),
                Some('"') => return Ok(out),
                Some('\\') => match self.bump() {
                    None => return Err(self.error_at(start, "this text has no closing `\"`")),
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some(escaped @ ('"' | '\\')) => out.push(escaped),
                    // Kept as written, so one odd escape doesn't fail the whole file.
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                },
                Some(character) => out.push(character),
            }
        }
    }

    fn unquoted(&mut self) -> Result<String, ParseError> {
        let start = self.pos;
        while let Some(character) = self.peek() {
            if character.is_whitespace() || matches!(character, '"' | '{' | '}') {
                break;
            }
            self.bump();
        }
        if self.pos == start {
            return Err(self.error("expected a key or a value"));
        }
        Ok(self.text[start..self.pos].to_owned())
    }

    /// Skips whitespace and `//` comments.
    fn skip_space(&mut self) {
        loop {
            match self.peek() {
                Some(character) if character.is_whitespace() => {
                    self.bump();
                }
                Some('/') if self.rest().starts_with("//") => {
                    while self.peek().is_some_and(|character| character != '\n') {
                        self.bump();
                    }
                }
                _ => return,
            }
        }
    }

    /// The conditional tags after a key or a value, like `[$WIN32]`.
    fn tags(&mut self) -> Result<Vec<String>, ParseError> {
        let mut tags = Vec::new();
        loop {
            self.skip_space();
            if self.peek() != Some('[') {
                return Ok(tags);
            }
            tags.push(self.tag()?);
        }
    }

    /// One `[...]` tag, without its brackets.
    fn tag(&mut self) -> Result<String, ParseError> {
        let start = self.pos;
        self.bump();
        let content = self.pos;
        loop {
            match self.bump() {
                Some(']') => return Ok(self.text[content..self.pos - 1].to_owned()),
                None | Some('\n') => {
                    return Err(self.error_at(start, "this `[` tag has no closing `]`"));
                }
                Some(_) => {}
            }
        }
    }

    fn rest(&self) -> &str {
        &self.text[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.pos += character.len_utf8();
        Some(character)
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        self.error_at(self.pos, message)
    }

    fn error_at(&self, offset: usize, message: impl Into<String>) -> ParseError {
        let before = &self.text[..offset];
        let line = before.matches('\n').count() + 1;
        let column = before
            .rsplit('\n')
            .next()
            .map_or(0, |last| last.chars().count())
            + 1;
        ParseError {
            line,
            column,
            message: message.into(),
        }
    }
}

/// Whether a tag's condition holds on Linux. Terms are `$NAME`, optionally
/// negated with `!`, and joined by `&&` or `||`. The joins apply left to right
/// with no precedence. Only `$LINUX` and `$POSIX` are true, so an unknown term
/// is false.
fn holds_on_linux(condition: &str) -> bool {
    let mut terms = Vec::new();
    let mut ands = Vec::new();
    let mut start = 0;
    let mut characters = condition.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        let doubled = matches!(character, '&' | '|')
            && characters
                .peek()
                .is_some_and(|&(_, next)| next == character);
        if doubled {
            terms.push(&condition[start..index]);
            ands.push(character == '&');
            characters.next();
            start = index + 2;
        }
    }
    terms.push(&condition[start..]);

    let mut value = term_holds(terms[0]);
    for (and, term) in ands.into_iter().zip(&terms[1..]) {
        let holds = term_holds(term);
        value = if and { value && holds } else { value || holds };
    }
    value
}

fn term_holds(term: &str) -> bool {
    let term = term.trim();
    let (negated, name) = match term.strip_prefix('!') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, term),
    };
    let holds = matches!(
        name.strip_prefix('$'),
        Some(name) if name.eq_ignore_ascii_case("LINUX") || name.eq_ignore_ascii_case("POSIX")
    );
    holds != negated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_blocks_keep_order_and_repeated_keys() {
        let object =
            parse(r#""root" { "a" "1" "inner" { "b" "2" "deeper" { "c" "3" } } "a" "4" }"#)
                .unwrap();
        let root = object.object("root").unwrap();
        assert_eq!(root.text("a"), Some("1"));
        let keys: Vec<&str> = root.entries().map(|(key, _)| key).collect();
        assert_eq!(keys, ["a", "inner", "a"]);
        assert_eq!(
            object
                .path(&["root", "inner", "deeper", "c"])
                .and_then(Value::as_text),
            Some("3")
        );
    }

    #[test]
    fn recognized_escapes_decode() {
        let cases = [
            ("\"v\" \"\\\"\"", "\""),
            ("\"v\" \"\\\\\"", "\\"),
            ("\"v\" \"\\n\"", "\n"),
            ("\"v\" \"\\t\"", "\t"),
            ("\"v\" \"\\r\"", "\r"),
        ];
        for (input, expected) in cases {
            assert_eq!(parse(input).unwrap().text("v"), Some(expected), "{input}");
        }
        let object = parse(r#""path" "C:\\Games\\\"Steam\"""#).unwrap();
        assert_eq!(object.text("path"), Some(r#"C:\Games\"Steam""#));
    }

    #[test]
    fn unknown_escapes_keep_their_backslash() {
        let object = parse(r#""odd" "keeps \q and \x41""#).unwrap();
        assert_eq!(object.text("odd"), Some(r"keeps \q and \x41"));
    }

    #[test]
    fn comments_are_skipped_but_not_inside_text() {
        let object = parse(
            "// header\n\"a\" \"http://x//y\" // trailing\n\"b\" { // inside\n \"c\" \"1\" }\n",
        )
        .unwrap();
        assert_eq!(object.text("a"), Some("http://x//y"));
        assert_eq!(object.object("b").unwrap().text("c"), Some("1"));
    }

    #[test]
    fn unquoted_tokens_work_as_keys_and_values() {
        let object = parse("name Steam\nlibrary { path /games/lib }").unwrap();
        assert_eq!(object.text("name"), Some("Steam"));
        assert_eq!(
            object.object("library").unwrap().text("path"),
            Some("/games/lib")
        );
    }

    #[test]
    fn competing_platform_entries_keep_the_linux_one_in_either_order() {
        let windows_first =
            parse(r#""path" "C:\\Games" [$WIN32] "path" "/mnt/games" [$LINUX]"#).unwrap();
        let linux_first =
            parse(r#""path" "/mnt/games" [$LINUX] "path" "C:\\Games" [$WIN32]"#).unwrap();
        for object in [&windows_first, &linux_first] {
            assert_eq!(object.text("path"), Some("/mnt/games"));
            assert_eq!(object.entries().count(), 1);
        }
    }

    #[test]
    fn negated_and_joined_conditions() {
        let object = parse(
            r#""not_windows" "1" [!$WIN32] "either" "2" [$WIN32||$LINUX] "both" "3" [$WIN32 && $LINUX] "spaced" "4" [ $LINUX || $OSX ] "left_to_right" "5" [$LINUX || $WIN32 && $WIN32]"#,
        )
        .unwrap();
        assert_eq!(object.text("not_windows"), Some("1"));
        assert_eq!(object.text("either"), Some("2"));
        assert_eq!(object.get("both"), None);
        assert_eq!(object.text("spaced"), Some("4"));
        assert_eq!(object.get("left_to_right"), None);
    }

    #[test]
    fn other_platform_and_unknown_conditions_drop_the_entry() {
        let object = parse(
            r#""osx" "1" [$OSX] "win64" "2" [$WIN64] "x360" "3" [$X360] "ps3" "4" [$PS3] "windows" "5" [$WINDOWS] "unknown" "6" [$SOMETHING] "kept" "7""#,
        )
        .unwrap();
        assert_eq!(object.entries().count(), 1);
        assert_eq!(object.text("kept"), Some("7"));
    }

    #[test]
    fn conditions_apply_to_object_values() {
        let object = parse(
            r#""block" [$WIN32] { "inner" "win" } "block" [$LINUX] { "inner" "linux" } "tail" { "x" "1" } [$OSX] "kept" [$POSIX] { "y" "2" }"#,
        )
        .unwrap();
        assert_eq!(object.object("block").unwrap().text("inner"), Some("linux"));
        assert_eq!(object.get("tail"), None);
        assert_eq!(object.object("kept").unwrap().text("y"), Some("2"));
        assert_eq!(object.entries().count(), 2);
    }

    #[test]
    fn entries_without_conditions_are_always_kept() {
        let object = parse(r#"[$WIN32] "plain" "1" "block" { "inner" "2" } [$LINUX]"#).unwrap();
        assert_eq!(object.text("plain"), Some("1"));
        assert_eq!(object.object("block").unwrap().text("inner"), Some("2"));
    }

    #[test]
    fn lookups_ignore_case() {
        let object =
            parse("\"AppState\" { \"StateFlags\" \"4\" \"installdir\" \"Game\" }").unwrap();
        assert_eq!(
            object.object("appstate").unwrap().text("stateflags"),
            Some("4")
        );
        assert_eq!(
            object
                .path(&["APPSTATE", "INSTALLDIR"])
                .and_then(Value::as_text),
            Some("Game")
        );
    }

    #[test]
    fn empty_files_and_byte_order_marks() {
        assert_eq!(parse("").unwrap(), Object::default());
        assert_eq!(parse("  \n// nothing here\n").unwrap(), Object::default());
        assert_eq!(parse("\u{feff}\"a\" \"b\"").unwrap().text("a"), Some("b"));
    }

    #[test]
    fn malformed_files_fail_with_a_position() {
        let cases = [
            ("\"a\" \"b", 1, 5, "no closing"),
            ("\"a\" \"b\\", 1, 5, "no closing"),
            ("\"a\"", 1, 4, "has no value"),
            ("\"a\" \"b\"\n\"c\"\n", 3, 1, "has no value"),
            ("\"a\" {\n \"b\" \"c\"\n", 1, 5, "never closed"),
            ("}", 1, 1, "unexpected `}`"),
            ("\"a\" \"b\" }", 1, 9, "unexpected `}`"),
            ("{ \"a\" \"b\" }", 1, 1, "expected a key"),
            ("\"a\" \"b\" [$WIN32", 1, 9, "no closing `]`"),
            ("\"a\" \"b\"\n\"c\" \"d\\", 2, 5, "no closing"),
        ];
        for (input, line, column, message) in cases {
            let error = parse(input).unwrap_err();
            assert_eq!((error.line, error.column), (line, column), "{input:?}");
            assert!(error.message.contains(message), "{input:?}: {error}");
            assert!(
                error
                    .to_string()
                    .starts_with(&format!("line {line}, column {column}"))
            );
        }
    }

    #[test]
    fn nesting_is_capped_without_a_panic() {
        let nest = |depth: usize| format!("{}{}", "\"k\" {".repeat(depth), "}".repeat(depth));
        assert!(parse(&nest(MAX_DEPTH)).is_ok());
        let error = parse(&nest(MAX_DEPTH + 1)).unwrap_err();
        assert!(error.message.contains("levels"), "{error}");
    }

    #[test]
    fn every_prefix_of_a_file_parses_or_fails_cleanly() {
        let sample = "\"AppState\"\n{\n\t\"appid\"\t\t\"1086940\" // id\n\t\"name\" \"Baldur's \\\"Gate\\\"\" [$WIN32]\n\t\"apps\" { \"228980\" \"1\" }\n}\n";
        for end in 0..=sample.len() {
            if sample.is_char_boundary(end) {
                let _ = parse(&sample[..end]);
            }
        }
        assert!(parse(sample).is_ok());
    }
}
