//! Capability and provenance checks for small source and Lean actions.

use crate::{
    goals::RequestStamp,
    protocol::{Position, Range},
};

#[derive(Clone, Debug, PartialEq)]
pub struct TextEdit {
    pub uri: String,
    pub version: i32,
    pub range: Range,
    pub new_text: String,
    pub generation: u64,
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
    if action.method != "Lean.Widget.getInteractiveGoals" {
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
