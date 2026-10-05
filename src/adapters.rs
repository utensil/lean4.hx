//! Bounded host-payload decoding with a trace-only fallback.

use serde_json::Value;

/// Do not let an unexpected host reply become an unbounded parse or render.
pub const MAX_ADAPTER_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub enum AdaptedPayload {
    Structured(Value),
    Fallback { bytes: usize, truncated: bool },
}

pub struct TextTraceAdapter;

impl TextTraceAdapter {
    pub fn decode(raw: &str) -> AdaptedPayload {
        if raw.len() > MAX_ADAPTER_BYTES {
            return AdaptedPayload::Fallback {
                bytes: raw.len(),
                truncated: true,
            };
        }
        match serde_json::from_str(raw) {
            Ok(value) => AdaptedPayload::Structured(value),
            Err(_) => AdaptedPayload::Fallback {
                bytes: raw.len(),
                truncated: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_json_is_preserved() {
        assert_eq!(
            TextTraceAdapter::decode(r#"{"goals":["⊢ True"]}"#),
            AdaptedPayload::Structured(serde_json::json!({"goals":["⊢ True"]}))
        );
    }

    #[test]
    fn malformed_text_falls_back_without_payload() {
        assert_eq!(
            TextTraceAdapter::decode("Lean callback failed"),
            AdaptedPayload::Fallback {
                bytes: 20,
                truncated: false
            }
        );
    }

    #[test]
    fn oversized_text_is_rejected_before_parsing() {
        let raw = "x".repeat(MAX_ADAPTER_BYTES + 1);
        assert_eq!(
            TextTraceAdapter::decode(&raw),
            AdaptedPayload::Fallback {
                bytes: MAX_ADAPTER_BYTES + 1,
                truncated: true
            }
        );
    }

    #[test]
    fn unicode_json_uses_byte_bound_without_losing_valid_data() {
        let raw = r#"{"message":"αβ🎉"}"#;
        assert!(matches!(
            TextTraceAdapter::decode(raw),
            AdaptedPayload::Structured(_)
        ));
    }
}
