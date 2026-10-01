//! Cursor-aware Lean Unicode input.
//!
//! The abbreviation corpus is data-driven so the editor does not silently
//! implement a small, divergent subset of Lean input. Templates use the
//! $CURSOR marker for paired delimiters and other insertions that leave the
//! cursor inside the replacement.

use std::{collections::BTreeMap, sync::OnceLock};

const ABBREVIATIONS: &str = include_str!("unicode-abbreviations.json");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnicodeEdit {
    pub start_chars: usize,
    pub end_chars: usize,
    pub replacement: String,
    pub cursor_chars: usize,
}

/// Convert a valid UTF-16 column into a Helix character column.
pub fn utf16_to_chars(line: &str, cursor_utf16: usize) -> Option<usize> {
    let mut utf16 = 0usize;
    for (chars, ch) in line.chars().enumerate() {
        if utf16 == cursor_utf16 {
            return Some(chars);
        }
        utf16 += ch.len_utf16();
    }
    (utf16 == cursor_utf16).then_some(line.chars().count())
}

fn abbreviations() -> &'static BTreeMap<String, String> {
    static TABLE: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    TABLE.get_or_init(|| {
        serde_json::from_str(ABBREVIATIONS)
            .expect("bundled Lean Unicode abbreviation data must be valid JSON")
    })
}

fn expand_template(template: &str) -> (String, usize) {
    if let Some(marker) = template.find("$CURSOR") {
        let prefix = &template[..marker];
        let suffix = &template[marker + "$CURSOR".len()..];
        (format!("{prefix}{suffix}"), prefix.chars().count())
    } else {
        let replacement = template.to_owned();
        let cursor = replacement.chars().count();
        (replacement, cursor)
    }
}

/// Find a supported Lean abbreviation immediately before a UTF-16 cursor.
/// Returned offsets are Unicode scalar offsets within `line`, matching
/// Helix's rope character ranges and avoiding split code points.
pub fn edit_at_cursor(line: &str, cursor_utf16: usize) -> Option<UnicodeEdit> {
    let mut utf16 = 0usize;
    let mut cursor_chars = 0usize;
    for ch in line.chars() {
        if utf16 >= cursor_utf16 {
            break;
        }
        utf16 += ch.len_utf16();
        cursor_chars += 1;
    }
    if utf16 != cursor_utf16 {
        return None;
    }

    let cursor_byte = line
        .char_indices()
        .nth(cursor_chars)
        .map_or(line.len(), |(byte, _)| byte);
    let before_cursor = &line[..cursor_byte];
    let token_end = before_cursor.trim_end_matches(char::is_whitespace).len();
    let token_start = before_cursor[..token_end]
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    let token = &before_cursor[token_start..token_end];
    let key = token.strip_prefix('\\')?;
    let template = abbreviations().get(key)?;
    let (replacement, cursor_chars_in_replacement) = expand_template(template);
    Some(UnicodeEdit {
        start_chars: before_cursor[..token_start].chars().count(),
        end_chars: before_cursor[..token_end].chars().count(),
        replacement,
        cursor_chars: cursor_chars_in_replacement,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_the_full_data_driven_corpus() {
        assert!(abbreviations().len() > 1_000);
        assert_eq!(abbreviations().get("alpha"), Some(&"α".to_owned()));
        assert_eq!(abbreviations().get("to"), Some(&"→".to_owned()));
    }

    #[test]
    fn expands_ascii_abbreviation_before_trailing_space() {
        assert_eq!(
            edit_at_cursor(r"x \alpha ", 9),
            Some(UnicodeEdit {
                start_chars: 2,
                end_chars: 8,
                replacement: "α".into(),
                cursor_chars: 1,
            })
        );
    }

    #[test]
    fn expands_pair_and_leaves_cursor_inside() {
        let edit = edit_at_cursor(r"\<> ", 4).unwrap();
        assert_eq!(edit.replacement, "⟨⟩");
        assert_eq!(edit.cursor_chars, 1);
    }

    #[test]
    fn preserves_unicode_prefix_and_utf16_cursor() {
        let line = "é \\to ";
        let cursor = line.encode_utf16().count();
        let edit = edit_at_cursor(line, cursor).unwrap();
        assert_eq!((edit.start_chars, edit.end_chars), (2, 5));
        assert_eq!(edit.replacement, "→");
    }

    #[test]
    fn rejects_unknown_or_mid_codepoint_cursor() {
        assert!(edit_at_cursor(r"\unknown ", 9).is_none());
        assert!(edit_at_cursor("é", 1).is_none());
    }
}
