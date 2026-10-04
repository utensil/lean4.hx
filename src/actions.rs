//! Capability and provenance checks for small source and Lean actions.

use crate::{
    goals::RequestStamp,
    protocol::{Position, Range},
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct TextEdit {
    pub uri: String,
    pub version: i32,
    pub range: Range,
    pub new_text: String,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceDocumentEdit {
    pub uri: String,
    pub version: Option<i32>,
    pub edits: Vec<TextEdit>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DocumentSnapshot {
    pub version: i32,
    pub generation: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RpcAction {
    pub uri: String,
    pub version: i32,
    pub generation: u64,
    pub session_id: String,
    pub method: String,
    pub params: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ActionError {
    Stale,
    UriMismatch,
    VersionMismatch,
    Unsupported(String),
    InvalidRange(String),
    MissingDocument(String),
    DuplicateDocument(String),
}

pub fn validate_text_edit(edit: &TextEdit, current: &RequestStamp) -> Result<(), ActionError> {
    if edit.generation != current.generation {
        return Err(ActionError::Stale);
    }
    if edit.uri != current.uri {
        return Err(ActionError::UriMismatch);
    }
    if edit.version != current.version {
        return Err(ActionError::VersionMismatch);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
struct PreparedTextEdit<'a> {
    start: usize,
    end: usize,
    new_text: &'a str,
}

/// Validate a single-document workspace edit without changing the source.
///
/// Every edit must belong to the same current URI, document version, and
/// lifecycle generation.  All UTF-16 ranges are resolved before success is
/// returned, and overlapping (or same-position) edits are rejected so callers
/// can apply the complete edit as one safe transaction.
pub fn preflight_workspace_edit(
    source: &str,
    edits: &[TextEdit],
    current: &RequestStamp,
) -> Result<(), ActionError> {
    prepare_workspace_edit(source, edits, current).map(|_| ())
}

/// Apply a validated single-document workspace edit atomically.
///
/// This is deliberately limited to one document: the provenance fields on
/// each [`TextEdit`] are checked against one [`RequestStamp`].  The complete
/// edit is prepared before any output is constructed, so a stale, malformed,
/// or overlapping member cannot leave a partially applied result.
pub fn apply_workspace_edit(
    source: &str,
    edits: &[TextEdit],
    current: &RequestStamp,
) -> Result<String, ActionError> {
    let prepared = prepare_workspace_edit(source, edits, current)?;
    let replacement_len = prepared
        .iter()
        .map(|edit| edit.new_text.len())
        .sum::<usize>();
    let mut result = String::with_capacity(source.len() + replacement_len);
    let mut cursor = 0usize;
    for edit in prepared {
        result.push_str(&source[cursor..edit.start]);
        result.push_str(edit.new_text);
        cursor = edit.end;
    }
    result.push_str(&source[cursor..]);
    Ok(result)
}

/// Preflight and apply a multi-document edit as one transaction. The returned
/// map is built only after every document and range has passed validation, so a
/// caller can publish all replacements together without partial writes.
pub fn apply_workspace_transaction(
    documents: &BTreeMap<String, DocumentSnapshot>,
    changes: &[WorkspaceDocumentEdit],
) -> Result<BTreeMap<String, String>, ActionError> {
    let mut result = BTreeMap::new();
    for change in changes {
        if result.contains_key(&change.uri) {
            return Err(ActionError::DuplicateDocument(change.uri.clone()));
        }
        let current = documents
            .get(&change.uri)
            .ok_or_else(|| ActionError::MissingDocument(change.uri.clone()))?;
        if let Some(version) = change.version {
            if version != current.version {
                return Err(ActionError::VersionMismatch);
            }
        }
        let stamp = RequestStamp {
            uri: change.uri.clone(),
            version: current.version,
            position: Position {
                line: 0,
                character: 0,
                extra: Default::default(),
            },
            generation: current.generation,
        };
        result.insert(
            change.uri.clone(),
            apply_workspace_edit(&current.text, &change.edits, &stamp)?,
        );
    }
    Ok(result)
}

fn prepare_workspace_edit<'a>(
    source: &str,
    edits: &'a [TextEdit],
    current: &RequestStamp,
) -> Result<Vec<PreparedTextEdit<'a>>, ActionError> {
    // Check provenance for every member before resolving any ranges.  This
    // keeps a mixed or stale workspace edit fail-closed as a whole.
    for edit in edits {
        validate_text_edit(edit, current)?;
    }

    let mut prepared = edits
        .iter()
        .map(|edit| {
            let start = byte_offset(source, &edit.range.start)?;
            let end = byte_offset(source, &edit.range.end)?;
            if start > end {
                return Err(ActionError::InvalidRange("start follows end".into()));
            }
            Ok(PreparedTextEdit {
                start,
                end,
                new_text: &edit.new_text,
            })
        })
        .collect::<Result<Vec<_>, ActionError>>()?;

    prepared.sort_by_key(|edit| (edit.start, edit.end));
    for pair in prepared.windows(2) {
        // A same-position pair is rejected even when one edit is an
        // insertion: LSP does not define an ordering for those replacements.
        if pair[1].start < pair[0].end || pair[1].start == pair[0].start {
            return Err(ActionError::InvalidRange("workspace edits overlap".into()));
        }
    }
    Ok(prepared)
}

pub fn validate_rpc_action(action: &RpcAction, current: &RequestStamp) -> Result<(), ActionError> {
    if action.generation != current.generation {
        return Err(ActionError::Stale);
    }
    if action.uri != current.uri {
        return Err(ActionError::UriMismatch);
    }
    if action.version != current.version {
        return Err(ActionError::VersionMismatch);
    }
    if !matches!(
        action.method.as_str(),
        "Lean.Widget.getInteractiveGoals" | "Lean.Widget.getInteractiveTermGoal"
    ) {
        return Err(ActionError::Unsupported(action.method.clone()));
    }
    if action.session_id.is_empty() {
        return Err(ActionError::Unsupported("missing RPC session".into()));
    }
    Ok(())
}

/// Apply one UTF-16 LSP edit after its provenance has been checked.
pub fn apply_text_edit(
    source: &str,
    range: &Range,
    replacement: &str,
) -> Result<String, ActionError> {
    let start = byte_offset(source, &range.start)?;
    let end = byte_offset(source, &range.end)?;
    if start > end {
        return Err(ActionError::InvalidRange("start follows end".into()));
    }
    let mut result = String::with_capacity(source.len() + replacement.len());
    result.push_str(&source[..start]);
    result.push_str(replacement);
    result.push_str(&source[end..]);
    Ok(result)
}

fn byte_offset(source: &str, position: &Position) -> Result<usize, ActionError> {
    let mut prefix = 0usize;
    let mut line = None;
    for (index, candidate) in source.split('\n').enumerate() {
        if index == position.line as usize {
            line = Some(candidate);
            break;
        }
        prefix += candidate.len() + 1;
    }
    let line =
        line.ok_or_else(|| ActionError::InvalidRange("line is outside the document".into()))?;
    let content = line.strip_suffix('\r').unwrap_or(line);
    let mut utf16 = 0u32;
    let mut bytes = 0usize;
    for ch in content.chars() {
        if utf16 == position.character {
            return Ok(prefix + bytes);
        }
        utf16 = utf16.saturating_add(ch.len_utf16() as u32);
        bytes += ch.len_utf8();
        if utf16 > position.character {
            return Err(ActionError::InvalidRange(
                "position splits a UTF-16 code unit".into(),
            ));
        }
    }
    if utf16 == position.character {
        return Ok(prefix + bytes);
    }
    Err(ActionError::InvalidRange(
        "character is outside the line".into(),
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    Active(u64),
    Cancelled,
    Closed,
    Restarted(u64),
    Reloaded(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryState {
    pub epoch: u64,
    pub lifecycle: Lifecycle,
}

impl Default for RecoveryState {
    fn default() -> Self {
        Self {
            epoch: 1,
            lifecycle: Lifecycle::Active(1),
        }
    }
}

impl RecoveryState {
    pub fn accepts(&self, generation: u64) -> bool {
        matches!(self.lifecycle, Lifecycle::Active(current) if current == generation)
    }

    pub fn cancel(&mut self) {
        self.lifecycle = Lifecycle::Cancelled;
    }
    pub fn close(&mut self) {
        self.lifecycle = Lifecycle::Closed;
    }
    pub fn restart(&mut self) {
        self.epoch = self.epoch.saturating_add(1);
        self.lifecycle = Lifecycle::Restarted(self.epoch);
        self.lifecycle = Lifecycle::Active(self.epoch);
    }
    pub fn reload(&mut self) {
        self.epoch = self.epoch.saturating_add(1);
        self.lifecycle = Lifecycle::Reloaded(self.epoch);
        self.lifecycle = Lifecycle::Active(self.epoch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Extras;

    fn stamp() -> RequestStamp {
        RequestStamp {
            uri: "file:///Main.lean".into(),
            version: 3,
            position: Position {
                line: 0,
                character: 0,
                extra: Extras::new(),
            },
            generation: 7,
        }
    }

    #[test]
    fn unicode_edit_uses_utf16_offsets() {
        let range = Range {
            start: Position {
                line: 0,
                character: 1,
                extra: Extras::new(),
            },
            end: Position {
                line: 0,
                character: 2,
                extra: Extras::new(),
            },
            extra: Extras::new(),
        };
        assert_eq!(apply_text_edit("aé\nβ", &range, "x").unwrap(), "ax\nβ");
    }

    #[test]
    fn workspace_edit_applies_unsorted_non_overlapping_ranges() {
        let current = stamp();
        let first = TextEdit {
            uri: current.uri.clone(),
            version: current.version,
            range: Range {
                start: Position {
                    line: 0,
                    character: 0,
                    extra: Extras::new(),
                },
                end: Position {
                    line: 0,
                    character: 1,
                    extra: Extras::new(),
                },
                extra: Extras::new(),
            },
            new_text: "x".into(),
            generation: current.generation,
        };
        let second = TextEdit {
            uri: current.uri.clone(),
            version: current.version,
            range: Range {
                start: Position {
                    line: 0,
                    character: 1,
                    extra: Extras::new(),
                },
                end: Position {
                    line: 0,
                    character: 2,
                    extra: Extras::new(),
                },
                extra: Extras::new(),
            },
            new_text: "y".into(),
            generation: current.generation,
        };
        let edits = [second, first];
        preflight_workspace_edit("aé\nβ", &edits, &current).unwrap();
        assert_eq!(
            apply_workspace_edit("aé\nβ", &edits, &current).unwrap(),
            "xy\nβ"
        );
    }

    #[test]
    fn workspace_edit_rejects_overlap_and_provenance_mismatch() {
        let current = stamp();
        let first = TextEdit {
            uri: current.uri.clone(),
            version: current.version,
            range: Range {
                start: Position {
                    line: 0,
                    character: 0,
                    extra: Extras::new(),
                },
                end: Position {
                    line: 0,
                    character: 2,
                    extra: Extras::new(),
                },
                extra: Extras::new(),
            },
            new_text: "x".into(),
            generation: current.generation,
        };
        let second = TextEdit {
            uri: current.uri.clone(),
            version: current.version,
            range: Range {
                start: Position {
                    line: 0,
                    character: 1,
                    extra: Extras::new(),
                },
                end: Position {
                    line: 0,
                    character: 2,
                    extra: Extras::new(),
                },
                extra: Extras::new(),
            },
            new_text: "y".into(),
            generation: current.generation,
        };
        assert_eq!(
            preflight_workspace_edit("abc", &[first.clone(), second.clone()], &current),
            Err(ActionError::InvalidRange("workspace edits overlap".into()))
        );
        assert_eq!(
            apply_workspace_edit("abc", &[first, second.clone()], &current),
            Err(ActionError::InvalidRange("workspace edits overlap".into()))
        );

        let mut stale = second.clone();
        stale.generation += 1;
        assert_eq!(
            preflight_workspace_edit("abc", &[stale], &current),
            Err(ActionError::Stale)
        );
        let mut wrong_version = second;
        wrong_version.version += 1;
        assert_eq!(
            preflight_workspace_edit("abc", &[wrong_version], &current),
            Err(ActionError::VersionMismatch)
        );
    }

    #[test]
    fn workspace_edit_preflights_every_range_before_returning() {
        let current = stamp();
        let valid = TextEdit {
            uri: current.uri.clone(),
            version: current.version,
            range: Range {
                start: Position {
                    line: 0,
                    character: 0,
                    extra: Extras::new(),
                },
                end: Position {
                    line: 0,
                    character: 1,
                    extra: Extras::new(),
                },
                extra: Extras::new(),
            },
            new_text: "x".into(),
            generation: current.generation,
        };
        let invalid = TextEdit {
            uri: current.uri.clone(),
            version: current.version,
            range: Range {
                start: Position {
                    line: 4,
                    character: 0,
                    extra: Extras::new(),
                },
                end: Position {
                    line: 4,
                    character: 0,
                    extra: Extras::new(),
                },
                extra: Extras::new(),
            },
            new_text: "y".into(),
            generation: current.generation,
        };
        let edits = [valid, invalid];
        let expected_preflight = Err(ActionError::InvalidRange(
            "line is outside the document".into(),
        ));
        assert_eq!(
            preflight_workspace_edit("abc", &edits, &current),
            expected_preflight
        );
        assert_eq!(
            apply_workspace_edit("abc", &edits, &current),
            Err(ActionError::InvalidRange(
                "line is outside the document".into()
            ))
        );
    }

    #[test]
    fn eof_positions_accept_empty_and_trailing_lines() {
        let empty = Range {
            start: Position {
                line: 0,
                character: 0,
                extra: Extras::new(),
            },
            end: Position {
                line: 0,
                character: 0,
                extra: Extras::new(),
            },
            extra: Extras::new(),
        };
        assert_eq!(apply_text_edit("", &empty, "x").unwrap(), "x");
        let trailing = Range {
            start: Position {
                line: 1,
                character: 0,
                extra: Extras::new(),
            },
            end: Position {
                line: 1,
                character: 0,
                extra: Extras::new(),
            },
            extra: Extras::new(),
        };
        assert_eq!(apply_text_edit("a\n", &trailing, "b").unwrap(), "a\nb");
        let crlf = Range {
            start: Position {
                line: 0,
                character: 1,
                extra: Extras::new(),
            },
            end: Position {
                line: 0,
                character: 1,
                extra: Extras::new(),
            },
            extra: Extras::new(),
        };
        assert_eq!(apply_text_edit("a\r\n", &crlf, "b").unwrap(), "ab\r\n");
    }

    #[test]
    fn stale_and_unsupported_actions_fail_closed() {
        let current = stamp();
        let edit = TextEdit {
            uri: current.uri.clone(),
            version: 3,
            range: Range {
                start: current.position.clone(),
                end: current.position.clone(),
                extra: Extras::new(),
            },
            new_text: "x".into(),
            generation: 6,
        };
        assert_eq!(validate_text_edit(&edit, &current), Err(ActionError::Stale));
        let action = RpcAction {
            uri: current.uri.clone(),
            version: 3,
            generation: 7,
            session_id: "1".into(),
            method: "unsupported".into(),
            params: serde_json::Value::Null,
        };
        assert_eq!(
            validate_rpc_action(&action, &current),
            Err(ActionError::Unsupported("unsupported".into()))
        );
        let mut term = action;
        term.method = "Lean.Widget.getInteractiveTermGoal".into();
        assert_eq!(validate_rpc_action(&term, &current), Ok(()));
    }

    #[test]
    fn workspace_transaction_preflights_all_documents_before_returning() {
        let mut documents = BTreeMap::new();
        documents.insert(
            "file:///Main.lean".into(),
            DocumentSnapshot {
                version: 4,
                generation: 8,
                text: "#check Nat\n".into(),
            },
        );
        documents.insert(
            "file:///Helper.lean".into(),
            DocumentSnapshot {
                version: 2,
                generation: 9,
                text: "def answer := 41\n".into(),
            },
        );
        let edit = |uri: &str, version: i32, generation: u64, text: &str| {
            WorkspaceDocumentEdit {
                uri: uri.into(),
                version: Some(version),
                edits: vec![TextEdit {
                    uri: uri.into(),
                    version,
                    range: Range {
                        start: Position {
                            line: 0,
                            character: 0,
                            extra: Default::default(),
                        },
                        end: Position {
                            line: 0,
                            character: 0,
                            extra: Default::default(),
                        },
                        extra: Default::default(),
                    },
                    new_text: text.into(),
                    generation,
                }],
            }
        };
        let changes = [
            edit("file:///Main.lean", 4, 8, "-- changed\n"),
            edit("file:///Helper.lean", 2, 9, "-- changed\n"),
        ];
        let applied = apply_workspace_transaction(&documents, &changes).unwrap();
        assert!(applied["file:///Main.lean"].starts_with("-- changed"));
        assert!(applied["file:///Helper.lean"].starts_with("-- changed"));

        let mut stale = changes.to_vec();
        stale[1].version = Some(3);
        assert_eq!(
            apply_workspace_transaction(&documents, &stale),
            Err(ActionError::VersionMismatch)
        );

        let duplicate = [changes[0].clone(), changes[0].clone()];
        assert_eq!(
            apply_workspace_transaction(&documents, &duplicate),
            Err(ActionError::DuplicateDocument("file:///Main.lean".into()))
        );
    }

    #[test]
    fn lifecycle_invalidates_old_work_on_cancel_close_restart_and_reload() {
        let mut state = RecoveryState::default();
        assert!(state.accepts(1));
        state.cancel();
        assert!(!state.accepts(1));
        state.restart();
        let generation = state.epoch;
        assert!(state.accepts(generation));
        state.close();
        assert!(!state.accepts(generation));
        state.reload();
        assert!(state.accepts(state.epoch));
    }
}
