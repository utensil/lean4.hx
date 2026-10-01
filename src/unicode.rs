//! Cursor-aware Lean Unicode abbreviations.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnicodeEdit {
    pub start_chars: usize,
    pub end_chars: usize,
    pub replacement: &'static str,
}

/// Find a supported Lean abbreviation immediately before a UTF-16 cursor.
/// The returned offsets are Unicode scalar offsets within `line`, which match
/// Helix's rope character ranges and avoid splitting a multi-byte character.
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
    let replacement = match token {
        r"\alpha" => "α",
        r"\beta" => "β",
        r"\gamma" => "γ",
        r"\delta" => "δ",
        r"\epsilon" => "ε",
        r"\lambda" | r"\lam" => "λ",
        r"\forall" => "∀",
        r"\exists" => "∃",
        r"\to" | r"\rightarrow" => "→",
        r"\mapsto" => "↦",
        r"\in" => "∈",
        r"\notin" => "∉",
        r"\ne" => "≠",
        r"\le" => "≤",
        r"\ge" => "≥",
        r"\times" => "×",
        _ => return None,
    };
    Some(UnicodeEdit {
        start_chars: line[..token_start].chars().count(),
        end_chars: line[..token_end].chars().count(),
        replacement,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_ascii_abbreviation_before_trailing_space() {
        assert_eq!(
            edit_at_cursor(r"x \alpha ", 9),
            Some(UnicodeEdit {
                start_chars: 2,
                end_chars: 8,
                replacement: "α",
            })
        );
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
