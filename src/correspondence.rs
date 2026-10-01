//! Source and goal correspondence without assuming that every location is local.

use crate::protocol::{Extras, Location, TaggedText};
use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, PartialEq)]
pub struct StyledSpan {
    pub text: String,
    pub tags: Vec<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalStyle {
    Plain,
    Keyword,
    Type,
    Goal,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerminalSpan {
    pub text: String,
    pub style: TerminalStyle,
}

/// Wrap by terminal cells, preserving grapheme clusters and style boundaries.
pub fn layout_lines(lines: &[Vec<TerminalSpan>], width: usize) -> Vec<Vec<Value>> {
    if width == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for line in lines {
        let mut row = Vec::new();
        let mut column = 0;
        for span in line {
            let style = match span.style {
                TerminalStyle::Plain => "plain",
                TerminalStyle::Keyword => "keyword",
                TerminalStyle::Type => "type",
                TerminalStyle::Goal => "goal",
                TerminalStyle::Error => "error",
            };
            for cluster in span.text.graphemes(true) {
                if cluster == "\n" || cluster == "\r\n" {
                    out.push(std::mem::take(&mut row));
                    column = 0;
                    continue;
                }
                let text = if cluster == "\t" { "    " } else { cluster };
                let cells = UnicodeWidthStr::width(text);
                if column + cells > width {
                    out.push(std::mem::take(&mut row));
                    column = 0;
                }
                if cells <= width {
                    row.push(serde_json::json!({"column":column,"text":text,"style":style}));
                    column += cells;
                }
            }
        }
        out.push(row);
    }
    out
}

fn style_for_tags(tags: &[Value]) -> TerminalStyle {
    for tag in tags.iter().rev() {
        if let Some(kind) = tag.get("kind").and_then(Value::as_str) {
            match kind {
                "keyword" | "tactic" => return TerminalStyle::Keyword,
                "type" | "typename" => return TerminalStyle::Type,
                "goal" | "target" => return TerminalStyle::Goal,
                "error" => return TerminalStyle::Error,
                _ => {}
            }
        }
        // Lean.Widget.getInteractiveGoals uses opaque info and subexpression
        // positions as its tag payload rather than a display-oriented kind.
        // Preserve that semantic boundary by giving tagged terms the type
        // style while leaving untagged punctuation plain.
        if tag.get("subexprPos").is_some() || tag.get("info").is_some() {
            return TerminalStyle::Type;
        }
    }
    TerminalStyle::Plain
}

/// Convert Lean tagged text into terminal spans, retaining unknown text while
/// collapsing adjacent fragments that have the same terminal style.
pub fn terminal_spans(value: &TaggedText) -> Vec<TerminalSpan> {
    let mut out: Vec<TerminalSpan> = Vec::new();
    for span in flatten_tagged_text(value) {
        let style = style_for_tags(&span.tags);
        if let Some(previous) = out.last_mut() {
            if previous.style == style {
                previous.text.push_str(&span.text);
                continue;
            }
        }
        out.push(TerminalSpan {
            text: span.text,
            style,
        });
    }
    out
}

pub fn flatten_tagged_text(value: &TaggedText) -> Vec<StyledSpan> {
    fn walk(value: &TaggedText, tags: &mut Vec<Value>, out: &mut Vec<StyledSpan>) {
        match value {
            TaggedText::Text(text) => out.push(StyledSpan {
                text: text.clone(),
                tags: tags.clone(),
            }),
            TaggedText::Append(values) => values.iter().for_each(|child| walk(child, tags, out)),
            TaggedText::Tag { tag, value } => {
                tags.push(tag.clone());
                walk(value, tags, out);
                tags.pop();
            }
            TaggedText::Unknown(value) => out.push(StyledSpan {
                text: value.to_string(),
                tags: tags.clone(),
            }),
        }
    }
    let mut out = Vec::new();
    walk(value, &mut Vec::new(), &mut out);
    out
}

#[derive(Clone, Debug, PartialEq)]
pub enum Navigation {
    Available(Location),
    Unavailable { uri: String, reason: String },
}

pub fn navigate(current_uri: &str, location: Location) -> Navigation {
    if location.uri.is_empty() {
        return Navigation::Unavailable {
            uri: current_uri.to_owned(),
            reason: "location has no URI".into(),
        };
    }
    Navigation::Available(location)
}

pub fn same_document(current_uri: &str, location: &Location) -> bool {
    current_uri == location.uri
}

/// Accept both LSP Location and LocationLink definition responses.
pub fn first_location(value: &Value) -> Option<Location> {
    if let Some(result) = value.get("result") {
        return first_location(result);
    }
    if let Some(values) = value.as_array() {
        return values.iter().find_map(first_location);
    }
    let object = value.as_object()?;
    if object.contains_key("uri") && object.contains_key("range") {
        return serde_json::from_value(value.clone()).ok();
    }
    let uri = object.get("targetUri")?.as_str()?.to_owned();
    let range = object
        .get("targetSelectionRange")
        .or_else(|| object.get("targetRange"))
        .cloned()?;
    Some(Location {
        uri,
        range: serde_json::from_value(range).ok()?,
        extra: Extras::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Extras, Position, Range};

    fn range() -> Range {
        Range {
            start: Position {
                line: 1,
                character: 2,
                extra: Extras::new(),
            },
            end: Position {
                line: 1,
                character: 7,
                extra: Extras::new(),
            },
            extra: Extras::new(),
        }
    }

    #[test]
    fn nested_tagged_text_flattens_without_discarding_tag_stack() {
        let value = TaggedText::Tag {
            tag: serde_json::json!({"kind": "goal"}),
            value: Box::new(TaggedText::Append(vec![
                TaggedText::Text("⊢ ".into()),
                TaggedText::Tag {
                    tag: serde_json::json!("target"),
                    value: Box::new(TaggedText::Text("Nat".into())),
                },
            ])),
        };
        let spans = flatten_tagged_text(&value);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "⊢ ");
        assert_eq!(spans[1].tags.len(), 2);
        assert_eq!(spans[1].text, "Nat");
    }

    #[test]
    fn terminal_spans_preserve_unknown_text_and_merge_styles() {
        let value = TaggedText::Append(vec![
            TaggedText::Tag {
                tag: serde_json::json!({"kind": "goal"}),
                value: Box::new(TaggedText::Text("⊢ ".into())),
            },
            TaggedText::Tag {
                tag: serde_json::json!({"kind": "goal"}),
                value: Box::new(TaggedText::Text("Nat".into())),
            },
            TaggedText::Unknown(serde_json::json!({"future": true})),
        ]);
        let spans = terminal_spans(&value);
        assert_eq!(spans[0].style, TerminalStyle::Goal);
        assert_eq!(spans[0].text, "⊢ Nat");
        assert_eq!(spans[1].style, TerminalStyle::Plain);
        assert!(spans[1].text.contains("future"));
    }

    #[test]
    fn locations_preserve_utf16_coordinates_and_cross_file_fallback() {
        let location = Location {
            uri: "file:///Helper.lean".into(),
            range: range(),
            extra: Extras::new(),
        };
        assert_eq!(location.range.start.character, 2);
        assert!(!same_document("file:///Main.lean", &location));
        assert_eq!(
            navigate("file:///Main.lean", location.clone()),
            Navigation::Available(location)
        );
        assert!(matches!(
            navigate(
                "file:///Main.lean",
                Location {
                    uri: String::new(),
                    range: range(),
                    extra: Extras::new()
                }
            ),
            Navigation::Unavailable { .. }
        ));
    }

    #[test]
    fn definition_locations_cover_location_and_location_link() {
        let direct = serde_json::json!([{"uri":"file:///Main.lean","range":{
            "start":{"line":1,"character":2},"end":{"line":1,"character":7}}}]);
        assert_eq!(first_location(&direct).unwrap().uri, "file:///Main.lean");
        let link = serde_json::json!({"result":[{"targetUri":"file:///Helper.lean",
            "targetRange":{"start":{"line":3,"character":4},"end":{"line":3,"character":10}}}]});
        let location = first_location(&link).unwrap();
        assert_eq!(location.uri, "file:///Helper.lean");
        assert_eq!(location.range.start.character, 4);
    }
}
