//! Argument tokenization matching Heroic's node-shlex dependency.
//! Reference: https://github.com/rgov/node-shlex/blob/de4f5017fd4bba9deb5869933b28faab286a2706/shlex.js

/// Parse arguments as data. Shell operators, comments, and variables stay literal.
pub fn parse(input: &str) -> Result<Vec<String>, String> {
    if input.contains('\0') {
        return Err("launcher arguments contain a NUL character".into());
    }
    let mut arguments = Vec::new();
    let mut token: Option<String> = None;
    let mut quote: Option<(char, bool)> = None;
    let mut escaped = false;
    let mut last_dollar = None;
    for (index, character) in input.chars().enumerate() {
        if escaped {
            if character != '\n' {
                let token = token.get_or_insert_default();
                if quote.is_some() {
                    token.push('\\');
                }
                token.push(character);
            }
            escaped = false;
            continue;
        }
        if character == '\\' && quote.is_none_or(|(mark, dollar)| mark == '"' || dollar) {
            escaped = true;
            continue;
        }
        if let Some((mark, dollar)) = quote {
            if character == mark {
                // node-shlex transforms the entire accumulated token at quote closure.
                let value = token.take().unwrap_or_default();
                token = Some(if mark == '\'' && dollar {
                    ansi_escapes(&value)?
                } else if mark == '"' {
                    double_escapes(&value)
                } else {
                    value
                });
                quote = None;
            } else {
                token.get_or_insert_default().push(character);
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            let dollar = last_dollar.is_some_and(|previous| previous + 1 == index);
            let token = token.get_or_insert_default();
            if dollar {
                token.pop();
            }
            quote = Some((character, dollar));
        } else if matches!(character, ' ' | '\t' | '\r' | '\n') {
            if let Some(token) = token.take() {
                arguments.push(token);
            }
        } else {
            if character == '$' {
                last_dollar = Some(index);
            }
            token.get_or_insert_default().push(character);
        }
    }
    if quote.is_some() {
        return Err("launcher arguments end inside a quoted string".into());
    }
    if escaped {
        return Err("launcher arguments end inside an escape sequence".into());
    }
    if let Some(token) = token {
        arguments.push(token);
    }
    Ok(arguments)
}

fn double_escapes(input: &str) -> String {
    let mut result = String::new();
    let mut characters = input.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\\'
            && characters
                .peek()
                .is_some_and(|next| matches!(next, '\\' | '"'))
        {
            result.push(characters.next().unwrap());
        } else {
            result.push(character);
        }
    }
    result
}

fn ansi_escapes(input: &str) -> Result<String, String> {
    let characters: Vec<_> = input.chars().collect();
    let mut units = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        index += 1;
        if character != '\\' || index == characters.len() {
            units.extend(character.encode_utf16(&mut [0; 2]).iter().copied());
            continue;
        }
        let next = characters[index];
        let value = match next {
            '\\' | '\'' | '"' | '?' => Some(next as u16),
            'a' => Some(7),
            'b' => Some(8),
            'e' | 'E' => Some(27),
            'f' => Some(12),
            'n' => Some(10),
            'r' => Some(13),
            't' => Some(9),
            'v' => Some(11),
            _ => None,
        };
        if let Some(value) = value {
            units.push(value);
            index += 1;
            continue;
        }
        let numeric = match next {
            '0'..='7' => Some((8, 3, index)),
            'x' => Some((16, 2, index + 1)),
            'u' => Some((16, 4, index + 1)),
            'U' => Some((16, 8, index + 1)),
            _ => None,
        };
        if let Some((radix, limit, start)) = numeric {
            let mut value = 0u32;
            let mut end = start;
            while end < characters.len() && end - start < limit {
                let Some(digit) = characters[end].to_digit(radix) else {
                    break;
                };
                value = value * radix + digit;
                end += 1;
            }
            if end > start {
                // node-shlex uses JavaScript UTF-16 code units for every numeric escape.
                units.push((value & 0xffff) as u16);
                index = end;
                continue;
            }
        }
        if next == 'c'
            && let Some(control) = characters.get(index + 1)
        {
            if matches!(control, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
                units.push('\\' as u16);
                continue;
            }
            if *control as u32 > 0xffff {
                return Err(
                    "non-BMP ANSI-C control escapes are unsupported; enter the argument literally"
                        .into(),
                );
            }
            units.push(if *control == '?' {
                127
            } else {
                (*control as u16) & 31
            });
            index += 2;
            continue;
        }
        units.push('\\' as u16);
    }
    if units.contains(&0) {
        return Err(
            "ANSI-C escape creates a NUL character, which cannot be passed as an argument".into(),
        );
    }
    String::from_utf16(&units).map_err(|_| {
        "ANSI-C escape creates an unpaired Unicode surrogate; enter valid Unicode text instead"
            .into()
    })
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn comments_operators_and_variables_remain_arguments() {
        assert_eq!(
            parse("--name #foo --bar $HOME x;y | 2>/dev/null").unwrap(),
            [
                "--name",
                "#foo",
                "--bar",
                "$HOME",
                "x;y",
                "|",
                "2>/dev/null"
            ]
        );
    }

    #[test]
    fn whitespace_and_empty_quoted_arguments_match_node_shlex() {
        assert_eq!(
            parse("  one\t\r\ntwo '' \"\" end").unwrap(),
            ["one", "two", "", "", "end"]
        );
        assert_eq!(
            parse("pre'joined text'post").unwrap(),
            ["prejoined textpost"]
        );
        assert!(parse(" \\ \"unclosed").is_err());
        assert!(parse("trailing\\").is_err());
        assert!(parse("\0").is_err());
    }

    #[test]
    fn escaping_does_not_apply_shell_expansion_rules() {
        assert_eq!(
            parse(r#"a\ b 'C:\Games\file' "C:\Games\file" "\$HOME \`command\` \"q\"" \# \$"#)
                .unwrap(),
            [
                "a b",
                r"C:\Games\file",
                r"C:\Games\file",
                r#"\$HOME \`command\` "q""#,
                "#",
                "$"
            ]
        );
        assert_eq!(
            parse("one\\\ntwo \"three\\\nfour\" '\\literal'").unwrap(),
            ["onetwo", "threefour", r"\literal"]
        );
        assert!(parse("\\\n").unwrap().is_empty());
    }

    #[test]
    fn locale_quotes_and_ansi_c_escapes_match_node_shlex() {
        assert_eq!(
            parse(r#"$"hello world" $'line\nnext\tend' $'\x41\101\u0042\U00000043'"#).unwrap(),
            ["hello world", "line\nnext\tend", "AABC"]
        );
        assert_eq!(
            parse(r"$'\a\b\e\E\f\r\v\cA\c?' $'\z\x' $'\uD83D\uDE00'").unwrap(),
            ["\x07\x08\x1b\x1b\x0c\r\x0b\x01\x7f", r"\z\x", "😀"]
        );
        assert_eq!(
            parse(r#"\$'literal' $'it\'s' $'' $"""#).unwrap(),
            ["$literal", "it's", "", ""]
        );
        assert_eq!(parse(r"$'\U0001F600'").unwrap(), ["\u{f600}"]);
    }

    #[test]
    fn unrepresentable_ansi_c_arguments_fail_clearly() {
        for input in [r"$'\0'", r"$'\c@'", r"$'\uD800'"] {
            assert!(parse(input).is_err(), "{input}");
        }
    }
    #[test]
    fn closing_quotes_reprocess_previous_segments_like_upstream() {
        assert_eq!(parse(r"'a\n'$'b\t'").unwrap(), ["a\nb\t"]);
        assert_eq!(parse(r#"'a\\b'"c""#).unwrap(), [r"a\bc"]);
        assert_eq!(parse(r#"'a\n'"c""#).unwrap(), [r"a\nc"]);
        assert_eq!(parse(r#"$'a\\n'"c""#).unwrap(), [r"a\nc"]);
    }
}
