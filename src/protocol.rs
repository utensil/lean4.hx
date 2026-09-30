//! Lossless, transport-level models for the standard LSP seam and Lean's
//! selected custom methods.  Unknown fields and payloads remain JSON values;
//! this module does not own sessions, documents, or editor state.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

pub type Extras = BTreeMap<String, Value>;

/// JSON-RPC identifiers retain their original JSON representation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(Number),
    String(String),
    Null,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(flatten)]
    pub extra: Extras,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: RequestId,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(flatten)]
    pub extra: Extras,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(flatten)]
    pub extra: Extras,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: RequestId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
    #[serde(flatten)]
    pub extra: Extras,
}

/// The three standard JSON-RPC envelope shapes.  Deserialization keeps the
/// selected shape while each member preserves fields it does not know.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Envelope {
    Request(Request),
    Notification(Notification),
    Response(Response),
}

/// A selected method name.  The original string and payload remain available
/// through [`MethodCall`] for forward-compatible pass-through.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeanMethod {
    PlainGoal,
    PlainTermGoal,
    RpcConnect,
    RpcKeepAlive,
    RpcRelease,
    Unknown,
}

impl LeanMethod {
    pub fn classify(method: &str) -> Self {
        match method {
            "$/lean/plainGoal" => Self::PlainGoal,
            "$/lean/plainTermGoal" => Self::PlainTermGoal,
            "$/lean/rpc/connect" => Self::RpcConnect,
            "$/lean/rpc/keepAlive" => Self::RpcKeepAlive,
            "$/lean/rpc/release" => Self::RpcRelease,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MethodCall {
    pub method: String,
    pub params: Value,
    pub kind: LeanMethod,
}

impl MethodCall {
    pub fn new(method: impl Into<String>, params: Value) -> Self {
        let method = method.into();
        let kind = LeanMethod::classify(&method);
        Self {
            method,
            params,
            kind,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
    #[serde(flatten)]
    pub extra: Extras,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
    #[serde(flatten)]
    pub extra: Extras,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
    #[serde(flatten)]
    pub extra: Extras,
}

/// Lean tagged-text records vary by server revision.  Known text is typed;
/// every tag or future field is retained in `extra`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaggedText {
    pub text: String,
    #[serde(flatten)]
    pub extra: Extras,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticRelatedInformation {
    pub location: Location,
    pub message: String,
    #[serde(flatten)]
    pub extra: Extras,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub range: Range,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(rename = "relatedInformation", skip_serializing_if = "Option::is_none")]
    pub related_information: Option<Vec<DiagnosticRelatedInformation>>,
    #[serde(flatten)]
    pub extra: Extras,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProgressToken {
    Number(Number),
    String(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    pub token: ProgressToken,
    pub value: Value,
    #[serde(flatten)]
    pub extra: Extras,
}

/// An intentionally opaque value for methods or payloads this version does
/// not understand.
pub type Opaque = Value;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_preserves_large_numeric_id_and_unknown_fields() {
        let source = json!({
            "jsonrpc": "2.0",
            "id": 9007199254740993_i64,
            "method": "$/lean/plainGoal",
            "params": {"nested": {"keep": true}},
            "future": {"flag": "opaque"}
        });
        let request: Request = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(
            request.id,
            RequestId::Number(Number::from(9007199254740993_i64))
        );
        assert_eq!(request.extra["future"], json!({"flag": "opaque"}));
        assert_eq!(serde_json::to_value(request).unwrap(), source);
    }

    #[test]
    fn selected_and_unknown_methods_round_trip_payloads() {
        assert_eq!(
            LeanMethod::classify("$/lean/rpc/keepAlive"),
            LeanMethod::RpcKeepAlive
        );
        let call = MethodCall::new("$/lean/future", json!({"deep": [1, {"x": null}]}));
        assert_eq!(call.kind, LeanMethod::Unknown);
        assert_eq!(call.params["deep"][1]["x"], Value::Null);
    }

    #[test]
    fn location_diagnostic_progress_and_tagged_text_keep_extensions() {
        let location: Location = serde_json::from_value(json!({
            "uri": "file:///workspace/Main.lean",
            "range": {
                "start": {"line": 1, "character": 2},
                "end": {"line": 1, "character": 4, "utf16": true}
            },
            "origin": "lean"
        }))
        .unwrap();
        assert_eq!(location.range.end.extra["utf16"], json!(true));
        assert_eq!(location.extra["origin"], json!("lean"));

        let diagnostic: Diagnostic = serde_json::from_value(json!({
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
            "message": "hello",
            "severity": 2,
            "relatedInformation": [],
            "tag": "unhandled"
        }))
        .unwrap();
        assert_eq!(diagnostic.extra["tag"], json!("unhandled"));

        let progress: Progress = serde_json::from_value(json!({
            "token": "lean",
            "value": {"kind": "report", "message": "working"},
            "extension": 7
        }))
        .unwrap();
        assert_eq!(progress.extra["extension"], json!(7));

        let tagged: TaggedText = serde_json::from_value(json!({
            "text": "Nat",
            "kind": "type",
            "attributes": ["opaque"]
        }))
        .unwrap();
        assert_eq!(tagged.extra["kind"], json!("type"));
    }

    #[test]
    fn response_keeps_error_and_extra_fields() {
        let response: Response = serde_json::from_value(json!({
            "jsonrpc": "2.0",
            "id": "x",
            "error": {"code": -32601, "message": "missing", "data": {"x": 1}, "future": true},
            "trace": "kept"
        }))
        .unwrap();
        assert_eq!(response.error.unwrap().extra["future"], json!(true));
        assert_eq!(response.extra["trace"], json!("kept"));
    }

    #[test]
    fn envelope_accepts_notification_without_inventing_an_id() {
        let value = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {"version": 1}}
        });
        let envelope: Envelope = serde_json::from_value(value).unwrap();
        assert!(matches!(envelope, Envelope::Notification(_)));
    }
}
