//! Source and goal correspondence without assuming that every location is local.

use crate::protocol::{Extras, Location, TaggedText};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct StyledSpan {
    pub text: String,
    pub tags: Vec<Value>,
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
