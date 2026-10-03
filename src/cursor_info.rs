//! Small, fail-closed parsers for cursor-scoped LSP information.
//!
//! The host receives hover, signature-help, inlay-hint, and plain-term-goal
//! values as open JSON.  This module keeps their rendering independent from
//! the host lifecycle: malformed payloads and values that do not belong to
//! the requested cursor simply produce no lines.

use crate::protocol::{PlainTermGoal, Position, Range};
use serde_json::Value;

/// Text projected into the cursor-information area of the native panel.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CursorInfo {
    pub lines: Vec<String>,
}

impl CursorInfo {
    /// Parse a response into cursor-information lines.
    pub fn parse(kind: &str, value: &Value, position: &Position) -> Option<Self> {
        parse(kind, value, position).map(|lines| Self { lines })
    }
}

/// Parse one cursor-information response into display lines.
///
/// `value` may be either the JSON-RPC result itself or an object containing a
/// `result` member.  The range on hover and term-goal responses is half-open;
/// a zero-length range is treated as a point at its exact position.
pub fn parse(kind: &str, value: &Value, position: &Position) -> Option<Vec<String>> {
    let result = result_value(value)?;
    match kind {
        "hover" | "textDocument/hover" => parse_hover(result, position),
        "signature" | "signature_help" | "signatureHelp" | "textDocument/signatureHelp" => {
            parse_signature(result)
        }
        "inlay" | "inlay_hint" | "inlayHint" | "textDocument/inlayHint" => {
            parse_inlay(result, position)
        }
        "term"
        | "plain_term_goal"
        | "plain-term-goal"
        | "plainTermGoal"
        | "$/lean/plainTermGoal" => parse_term_goal(result, position),
        _ => None,
    }
}

fn result_value(value: &Value) -> Option<&Value> {
    if let Some(result) = value.get("result") {
        return (!result.is_null()).then_some(result);
    }
    (!value.is_null()).then_some(value)
}

fn parse_hover(value: &Value, position: &Position) -> Option<Vec<String>> {
    if let Some(raw_range) = value.get("range") {
        let range = valid_range(raw_range)?;
        if !contains(position, &range) {
            return None;
        }
    }
    let contents = value.get("contents")?;
    nonempty(text_lines(contents, true))
}

fn parse_signature(value: &Value) -> Option<Vec<String>> {
    let signatures = value.get("signatures")?.as_array()?;
    let active = value
        .get("activeSignature")
        .and_then(Value::as_u64)
        .and_then(|index| usize::try_from(index).ok())
        .unwrap_or(0);
    let signature = signatures
        .get(active)
        .or_else(|| signatures.first())?
        .as_object()?;

    let mut lines = text_lines(signature.get("label")?, true);
    if let Some(documentation) = signature.get("documentation") {
        lines.extend(text_lines(documentation, true));
    }
    nonempty(lines)
}

fn parse_inlay(value: &Value, position: &Position) -> Option<Vec<String>> {
    let hints = if let Some(hints) = value.as_array() {
        hints.iter().collect::<Vec<_>>()
    } else if let Some(hints) = value.get("items").and_then(Value::as_array) {
        hints.iter().collect::<Vec<_>>()
    } else if value.get("position").is_some() {
        vec![value]
    } else {
        return None;
    };

    let mut lines = Vec::new();
    for hint in hints {
        let Some(object) = hint.as_object() else {
            continue;
        };
        let Some(raw_position) = object.get("position") else {
            continue;
        };
        let Ok(hint_position) = serde_json::from_value::<Position>(raw_position.clone()) else {
            continue;
        };
        if compare(&hint_position, position) != std::cmp::Ordering::Equal {
            continue;
        }
        let Some(label) = object.get("label") else {
            continue;
        };
        let Some(label) = inlay_label(label) else {
            continue;
        };
        lines.extend(text_lines(&Value::String(label), false));
    }
    nonempty(lines)
}

fn parse_term_goal(value: &Value, position: &Position) -> Option<Vec<String>> {
    let goal: PlainTermGoal = serde_json::from_value(value.clone()).ok()?;
    let range = valid_range(&serde_json::to_value(&goal.range).ok()?)?;
    if !contains(position, &range) {
        return None;
    }
    nonempty(text_lines(&Value::String(goal.goal), false))
}

/// Parse an LSP range and reject both malformed and backwards ranges.
fn valid_range(value: &Value) -> Option<Range> {
    let range = serde_json::from_value::<Range>(value.clone()).ok()?;
    (compare(&range.start, &range.end) != std::cmp::Ordering::Greater).then_some(range)
}

fn contains(position: &Position, range: &Range) -> bool {
    match compare(&range.start, &range.end) {
        std::cmp::Ordering::Greater => false,
        std::cmp::Ordering::Equal => compare(position, &range.start) == std::cmp::Ordering::Equal,
        std::cmp::Ordering::Less => {
            compare(&range.start, position) != std::cmp::Ordering::Greater
                && compare(position, &range.end) == std::cmp::Ordering::Less
        }
    }
}

fn compare(left: &Position, right: &Position) -> std::cmp::Ordering {
    left.line
        .cmp(&right.line)
        .then_with(|| left.character.cmp(&right.character))
}

fn nonempty(lines: Vec<String>) -> Option<Vec<String>> {
    (!lines.is_empty()).then_some(lines)
}

/// Extract text from MarkedString, MarkupContent, or an array of either.
fn text_lines(value: &Value, strip_fences: bool) -> Vec<String> {
    match value {
        Value::String(text) => split_text(text, strip_fences),
        Value::Array(values) => values
            .iter()
            .flat_map(|value| text_lines(value, strip_fences))
            .collect(),
        Value::Object(object) => {
            // MarkedString `{language, value}` and MarkupContent
            // `{kind, value}` share the same useful payload.
            if let Some(value) = object.get("value") {
                return text_lines(value, strip_fences);
            }
            // A few clients wrap documentation or contents one level deeper.
            for key in ["label", "documentation", "contents"] {
                if let Some(value) = object.get(key) {
                    return text_lines(value, strip_fences);
                }
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// Inlay labels may be one string or an array of label-part objects.
fn inlay_label(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => (!text.is_empty()).then_some(text.clone()),
        Value::Array(parts) => {
            let mut text = String::new();
            for part in parts {
                match part {
                    Value::String(part) => text.push_str(part),
                    Value::Object(object) => text.push_str(object.get("value")?.as_str()?),
                    _ => return None,
                }
            }
            (!text.is_empty()).then_some(text)
        }
        Value::Object(object) => object.get("value")?.as_str().map(str::to_owned),
        _ => None,
    }
}

/// Split text without trimming its meaningful whitespace.  Markdown fence
/// lines and empty outer formatting lines are removed, while internal blank
/// lines and leading/trailing spaces on content lines are retained.
fn split_text(text: &str, strip_fences: bool) -> Vec<String> {
    let mut lines = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
        .collect::<Vec<_>>();
    if strip_fences {
        lines.retain(|line| !is_fence(line));
    }
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines
}

fn is_fence(line: &str) -> bool {
    line.trim_start().starts_with("```")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Extras;
    use serde_json::json;

    fn position(line: u32, character: u32) -> Position {
        Position {
            line,
            character,
            extra: Extras::new(),
        }
    }

    #[test]
    fn hover_marked_string_strips_fences_and_keeps_spacing() {
        let value = json!({
            "contents": [
                {"language": "lean", "value": "```lean\n  Nat  \n\n```"},
                {"kind": "markdown", "value": " docs "}
            ],
            "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 3}}
        });
        assert_eq!(
            parse("hover", &value, &position(1, 2)),
            Some(vec!["  Nat  ".into(), " docs ".into()])
        );
    }

    #[test]
    fn hover_range_is_half_open_but_point_range_is_exact() {
        let value = json!({
            "contents": "x",
            "range": {"start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 3}}
        });
        assert!(parse("hover", &value, &position(0, 3)).is_none());
        assert!(parse("hover", &value, &position(0, 1)).is_some());

        let point = json!({
            "contents": "x",
            "range": {"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 4}}
        });
        assert!(parse("hover", &point, &position(2, 4)).is_some());
        assert!(parse("hover", &point, &position(2, 5)).is_none());
    }

    #[test]
    fn malformed_or_backwards_hover_range_is_rejected() {
        let malformed = json!({"contents": "x", "range": {"start": {"line": 0}}});
        assert!(parse("hover", &malformed, &position(0, 0)).is_none());
        let backwards = json!({
            "contents": "x",
            "range": {"start": {"line": 1, "character": 0}, "end": {"line": 0, "character": 0}}
        });
        assert!(parse("hover", &backwards, &position(0, 0)).is_none());
    }

    #[test]
    fn signature_uses_active_label_and_documentation() {
        let value = json!({
            "signatures": [
                {"label": "old"},
                {"label": "f (x : Nat)", "documentation": {"kind": "markdown", "value": "```\nmeaning\n```"}}
            ],
            "activeSignature": 1
        });
        assert_eq!(
            parse("signature", &value, &position(0, 0)),
            Some(vec!["f (x : Nat)".into(), "meaning".into()])
        );
    }

    #[test]
    fn inlay_keeps_only_exact_cursor_and_joins_label_parts() {
        let value = json!([
            {"position": {"line": 1, "character": 2}, "label": " : Nat"},
            {"position": {"line": 1, "character": 3}, "label": " outside"},
            {"position": {"line": 1, "character": 2}, "label": [{"value": " := "}, {"value": "x"}]}
        ]);
        assert_eq!(
            parse("inlay", &value, &position(1, 2)),
            Some(vec![" : Nat".into(), " := x".into()])
        );
        assert!(parse("inlay", &value, &position(1, 9)).is_none());
    }

    #[test]
    fn plain_term_goal_requires_cursor_in_range() {
        let value = json!({
            "goal": "x : Nat\n⊢ x = x",
            "range": {"start": {"line": 2, "character": 1}, "end": {"line": 2, "character": 5}}
        });
        assert_eq!(
            parse("plainTermGoal", &value, &position(2, 1)),
            Some(vec!["x : Nat".into(), "⊢ x = x".into()])
        );
        assert!(parse("plainTermGoal", &value, &position(2, 5)).is_none());
    }
}
