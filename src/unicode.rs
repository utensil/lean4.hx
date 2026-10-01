//! Cursor-aware Lean Unicode input.
//!
//! The abbreviation corpus is data-driven so the editor does not silently
//! implement a small, divergent subset of Lean input. Templates use the
//! `$CURSOR` marker for paired delimiters and other insertions that leave the
//! cursor inside the replacement.
//!
//! `unicode-abbreviations.json` is vendored from the public Lean VS Code
//! abbreviation corpus at revision `dead846a035f42dc13beb7619ac779538e6ddf6e`.
//! See `src/unicode-abbreviations.license` and
//! `scripts/sync-unicode-abbreviations.sh` for attribution and reproducible
//! refresh instructions.

use serde::de::{MapAccess, Visitor};
use std::{collections::BTreeMap, fmt, sync::OnceLock};

const ABBREVIATIONS: &str = include_str!("unicode-abbreviations.json");
const CURSOR_MARKER: &str = "$CURSOR";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnicodeEdit {
    pub start_chars: usize,
    pub end_chars: usize,
    pub replacement: String,
    pub cursor_chars: usize,
}

/// A completion from the bundled abbreviation corpus.
///
/// `abbreviation` excludes the leading backslash. `replacement` has its
/// `$CURSOR` marker removed, and `cursor_chars` is the character offset of the
/// marker (or the end of the replacement when the template has no marker).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnicodeCandidate {
    pub abbreviation: String,
    pub replacement: String,
    pub cursor_chars: usize,
}

impl UnicodeCandidate {
    /// The corpus key, excluding the input leader.
    pub fn key(&self) -> &str {
        &self.abbreviation
    }

    /// The replacement text, excluding the `$CURSOR` marker.
    pub fn symbol(&self) -> &str {
        &self.replacement
    }
}

struct AbbreviationCorpus {
    /// A sorted view for exact lookup and deterministic iteration.
    map: BTreeMap<String, String>,
    /// The source order is retained for shortest-prefix replacement. The
    /// upstream Lean input provider uses declaration order to break ties.
    ordered: Vec<(String, String)>,
}

impl<'de> serde::Deserialize<'de> for AbbreviationCorpus {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CorpusVisitor;

        impl<'de> Visitor<'de> for CorpusVisitor {
            type Value = AbbreviationCorpus;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object containing Unicode abbreviation templates")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut ordered = Vec::new();
                while let Some((key, value)) = access.next_entry::<String, String>()? {
                    ordered.push((key, value));
                }
                let map = ordered.iter().cloned().collect();
                Ok(AbbreviationCorpus { map, ordered })
            }
        }

        deserializer.deserialize_map(CorpusVisitor)
    }
}

fn corpus() -> &'static AbbreviationCorpus {
    static CORPUS: OnceLock<AbbreviationCorpus> = OnceLock::new();
    CORPUS.get_or_init(|| {
        serde_json::from_str(ABBREVIATIONS)
            .expect("bundled Lean Unicode abbreviation data must be valid JSON")
    })
}

fn abbreviations() -> &'static BTreeMap<String, String> {
    &corpus().map
}

fn expand_template(template: &str) -> (String, usize) {
    if let Some(marker) = template.find(CURSOR_MARKER) {
        let prefix = &template[..marker];
        let suffix = &template[marker + CURSOR_MARKER.len()..];
        (format!("{prefix}{suffix}"), prefix.chars().count())
    } else {
        let replacement = template.to_owned();
        let cursor = replacement.chars().count();
        (replacement, cursor)
    }
}

fn candidate(abbreviation: &str, template: &str) -> UnicodeCandidate {
    let (replacement, cursor_chars) = expand_template(template);
    UnicodeCandidate {
        abbreviation: abbreviation.to_owned(),
        replacement,
        cursor_chars,
    }
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

fn cursor_position(line: &str, cursor_utf16: usize) -> Option<(usize, usize)> {
    let mut utf16 = 0usize;
    for (char_index, (byte_index, ch)) in line.char_indices().enumerate() {
        if utf16 == cursor_utf16 {
            return Some((char_index, byte_index));
        }
        utf16 += ch.len_utf16();
        if utf16 > cursor_utf16 {
            return None;
        }
    }
    (utf16 == cursor_utf16).then_some((line.chars().count(), line.len()))
}

fn normalized_prefix(prefix: &str) -> &str {
    prefix.strip_prefix('\\').unwrap_or(prefix)
}

/// Return all corpus entries whose abbreviation starts with `prefix`.
///
/// The leading backslash is optional. Results are deterministic: an exact
/// match is first, followed by candidates ordered by abbreviation length and
/// then lexicographically. Keeping the exact entry first makes a completion
/// list useful both for typed prefixes and for an already complete key.
pub fn autocomplete_candidates(prefix: &str) -> Vec<UnicodeCandidate> {
    let prefix = normalized_prefix(prefix);
    let mut candidates: Vec<_> = corpus()
        .map
        .iter()
        .filter(|(abbreviation, _)| abbreviation.starts_with(prefix))
        .map(|(abbreviation, template)| candidate(abbreviation, template))
        .collect();
    candidates.sort_by(|left, right| {
        let left_exact = left.abbreviation == prefix;
        let right_exact = right.abbreviation == prefix;
        right_exact
            .cmp(&left_exact)
            .then_with(|| {
                left.abbreviation
                    .chars()
                    .count()
                    .cmp(&right.abbreviation.chars().count())
            })
            .then_with(|| left.abbreviation.cmp(&right.abbreviation))
    });
    candidates
}

/// Alias retained for callers that describe these as abbreviation candidates.
pub fn abbreviation_candidates(prefix: &str) -> Vec<UnicodeCandidate> {
    autocomplete_candidates(prefix)
}

/// Return candidate keys for a prefix, without the leading backslash.
pub fn abbreviation_keys(prefix: &str) -> Vec<String> {
    autocomplete_candidates(prefix)
        .into_iter()
        .map(|candidate| candidate.abbreviation)
        .collect()
}

/// Return replacement texts for a prefix in the same order as
/// [`autocomplete_candidates`]. Duplicate symbols are retained because
/// aliases are distinct corpus entries.
pub fn find_symbols_by_abbreviation_prefix(prefix: &str) -> Vec<String> {
    autocomplete_candidates(prefix)
        .into_iter()
        .map(|candidate| candidate.replacement)
        .collect()
}

/// Return the shortest corpus replacement matching a typed abbreviation.
///
/// This mirrors Lean's manual replacement behavior: a complete key is not
/// required, and the shortest matching key wins. If no key starts with the
/// complete input, the longest usable prefix is replaced and the remainder is
/// kept verbatim (for example, `alp7` becomes `α7`).
pub fn replacement_for_abbreviation(abbreviation: &str) -> Option<String> {
    let abbreviation = normalized_prefix(abbreviation);
    if abbreviation.is_empty() {
        return None;
    }

    let matching = corpus()
        .ordered
        .iter()
        .enumerate()
        .filter(|(_, (key, _))| key.starts_with(abbreviation))
        .min_by_key(|(index, (key, _))| (key.chars().count(), *index));
    if let Some((_, (_, template))) = matching {
        return Some(expand_template(template).0);
    }

    let mut chars = abbreviation.chars();
    let last = chars.next_back()?;
    let prefix: String = chars.collect();
    let replacement = replacement_for_abbreviation(&prefix)?;
    Some(format!("{replacement}{last}"))
}

/// Return the exact entry when it is the only corpus key matching `prefix`.
/// This is the eager replacement rule used by Lean's editor integration.
pub fn unique_abbreviation(prefix: &str) -> Option<UnicodeCandidate> {
    let prefix = normalized_prefix(prefix);
    let mut matches = corpus()
        .map
        .iter()
        .filter(|(abbreviation, _)| abbreviation.starts_with(prefix));
    let first = matches.next()?;
    if matches.next().is_some() || first.0 != prefix {
        return None;
    }
    Some(candidate(first.0, first.1))
}

/// Whether `prefix` is a complete, unique abbreviation.
pub fn is_unique_abbreviation(prefix: &str) -> bool {
    unique_abbreviation(prefix).is_some()
}

#[derive(Clone, Debug)]
struct AbbreviationMatch {
    slash_char: usize,
    end_char: usize,
    abbreviation: String,
}

fn is_delimiter(ch: char) -> bool {
    ch.is_whitespace() || (!ch.is_alphanumeric() && ch != '_')
}

fn slash_is_escaped(chars: &[(usize, char)], slash_char: usize) -> bool {
    let mut preceding = 0usize;
    let mut index = slash_char;
    while index > 0 && chars[index - 1].1 == '\\' {
        preceding += 1;
        index -= 1;
    }
    preceding % 2 == 1
}

fn match_suffix(suffix: &str) -> Option<(usize, String)> {
    let suffix_chars: Vec<char> = suffix.chars().collect();
    if suffix_chars.is_empty() {
        return None;
    }

    // Prefer the longest complete key at the beginning of the suffix. This
    // handles both punctuation (`\\alpha,`) and templates with a trailing
    // input character (`\\+ `) without discarding the corpus's special keys.
    let mut best: Option<(usize, String)> = None;
    for (key, _) in abbreviations() {
        let key_chars: Vec<char> = key.chars().collect();
        if key_chars.is_empty() || key_chars.len() > suffix_chars.len() {
            continue;
        }
        if suffix_chars[..key_chars.len()] != key_chars[..] {
            continue;
        }
        let remainder = &suffix_chars[key_chars.len()..];
        let allowed = remainder.is_empty()
            || remainder.first().is_some_and(|ch| is_delimiter(*ch))
            // A corpus backslash key is an explicit escape for a literal
            // slash, so `\\alpha` reduces to `\\alpha` after the pair.
            || key == "\\";
        if !allowed {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|(length, _)| key_chars.len() > *length)
        {
            best = Some((key_chars.len(), key.clone()));
        }
    }
    best
}

fn abbreviation_at_cursor(line: &str, cursor_utf16: usize) -> Option<AbbreviationMatch> {
    let (cursor_char, cursor_byte) = cursor_position(line, cursor_utf16)?;
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    for slash_char in (0..cursor_char).rev() {
        if chars[slash_char].1 != '\\' || slash_is_escaped(&chars, slash_char) {
            continue;
        }
        let slash_byte = chars[slash_char].0;
        let suffix = &line[slash_byte + '\\'.len_utf8()..cursor_byte];
        let Some((key_len, abbreviation)) = match_suffix(suffix) else {
            continue;
        };
        return Some(AbbreviationMatch {
            slash_char,
            end_char: slash_char + 1 + key_len,
            abbreviation,
        });
    }
    None
}

/// Find a supported Lean abbreviation immediately before a UTF-16 cursor.
/// Returned offsets are Unicode scalar offsets within `line`, matching
/// Helix's rope character ranges and avoiding split code points.
pub fn edit_at_cursor(line: &str, cursor_utf16: usize) -> Option<UnicodeEdit> {
    let found = abbreviation_at_cursor(line, cursor_utf16)?;
    let template = abbreviations().get(&found.abbreviation)?;
    let (replacement, cursor_chars_in_replacement) = expand_template(template);
    Some(UnicodeEdit {
        start_chars: found.slash_char,
        end_chars: found.end_char,
        replacement,
        cursor_chars: cursor_chars_in_replacement,
    })
}

/// Expand only an exact abbreviation that is uniquely identified by its
/// prefix. This helper is useful to hosts that implement eager replacement
/// while leaving [`edit_at_cursor`] suitable for explicit delimiter-triggered
/// expansion.
pub fn unique_edit_at_cursor(line: &str, cursor_utf16: usize) -> Option<UnicodeEdit> {
    let found = abbreviation_at_cursor(line, cursor_utf16)?;
    let candidate = unique_abbreviation(&found.abbreviation)?;
    Some(UnicodeEdit {
        start_chars: found.slash_char,
        end_chars: found.end_char,
        replacement: candidate.replacement,
        cursor_chars: candidate.cursor_chars,
    })
}

/// Alias for [`unique_edit_at_cursor`] using the terminology of eager input.
pub fn eager_edit_at_cursor(line: &str, cursor_utf16: usize) -> Option<UnicodeEdit> {
    unique_edit_at_cursor(line, cursor_utf16)
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

    #[test]
    fn supports_literal_slash_and_slash_escaped_text() {
        let escaped = edit_at_cursor(r"\\ ", 3).unwrap();
        assert_eq!((escaped.start_chars, escaped.end_chars), (0, 2));
        assert_eq!(escaped.replacement, "\\");

        let prefix = edit_at_cursor(r"\\alpha ", 8).unwrap();
        assert_eq!((prefix.start_chars, prefix.end_chars), (0, 2));
        assert_eq!(prefix.replacement, "\\");
    }

    #[test]
    fn retains_the_trailing_space_corpus_key() {
        let edit = edit_at_cursor(r"\+ ", 3).unwrap();
        assert_eq!((edit.start_chars, edit.end_chars), (0, 3));
        assert_eq!(edit.replacement, "⊹");
    }

    #[test]
    fn punctuation_ends_an_abbreviation_without_consuming_it() {
        let edit = edit_at_cursor(r"(\alpha), ", 10).unwrap();
        assert_eq!((edit.start_chars, edit.end_chars), (1, 7));
        assert_eq!(edit.replacement, "α");
    }

    #[test]
    fn autocomplete_is_exact_first_and_deterministic() {
        let candidates = autocomplete_candidates("alpha");
        assert_eq!(candidates.first().unwrap().abbreviation, "alpha");
        assert_eq!(autocomplete_candidates("\\alpha"), candidates);
        let keys: Vec<_> = autocomplete_candidates("al")
            .into_iter()
            .map(|candidate| candidate.abbreviation)
            .collect();
        assert_eq!(
            keys,
            vec!["all", "allf", "allm", "aleph", "all^f", "all^m", "alpha", "aleph0", "alghom"]
        );
    }

    #[test]
    fn replacement_and_unique_matching_follow_lean_rules() {
        assert_eq!(replacement_for_abbreviation("alp"), Some("α".into()));
        assert_eq!(replacement_for_abbreviation(r"\alp"), Some("α".into()));
        assert_eq!(replacement_for_abbreviation("alp7"), Some("α7".into()));
        assert!(is_unique_abbreviation("alpha"));
        assert!(!is_unique_abbreviation("a"));
        assert!(unique_edit_at_cursor(r"\alpha ", 7).is_some());
        assert!(unique_edit_at_cursor(r"\a ", 3).is_none());
    }

    #[test]
    fn templates_report_the_cursor_inside_pairs() {
        let candidate = autocomplete_candidates("<>").pop().unwrap();
        assert_eq!(candidate.replacement, "⟨⟩");
        assert_eq!(candidate.cursor_chars, 1);
    }

    #[test]
    fn every_bundled_key_is_selectable_at_a_delimited_cursor() {
        for (key, template) in &corpus().ordered {
            let line = format!("\\{key} ");
            let cursor = line.encode_utf16().count();
            let edit = edit_at_cursor(&line, cursor)
                .unwrap_or_else(|| panic!("missing edit for corpus key {key:?}"));
            let (replacement, cursor_chars) = expand_template(template);
            assert_eq!(edit.replacement, replacement, "key {key:?}");
            assert_eq!(edit.cursor_chars, cursor_chars, "key {key:?}");
        }
    }
}
