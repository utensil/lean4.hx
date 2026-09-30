//! Lossless, transport-level models for JSON-RPC and Lean's selected methods.
//!
//! Open-ended Lean payloads remain `serde_json::Value`; JSON `null` is distinct
//! from an omitted member and JSON numbers are never converted through `f64`.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Number, Value};
use std::collections::BTreeMap;

pub type Extras = BTreeMap<String, Value>;

/// A member that can be absent, explicitly null, or contain a value.
#[derive(Clone, Debug, PartialEq)]
pub enum Presence<T> {
    Absent,
    Null,
    Value(T),
}
impl<T> Default for Presence<T> {
    fn default() -> Self {
        Self::Absent
    }
}
impl<T> Presence<T> {
    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
    pub fn as_ref(&self) -> Option<&T> {
        if let Self::Value(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn into_option(self) -> Option<T> {
        if let Self::Value(v) = self {
            Some(v)
        } else {
            None
        }
    }
}
impl<T: Serialize> Serialize for Presence<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Absent => serializer.serialize_unit(),
            Self::Null => serializer.serialize_none(),
            Self::Value(v) => v.serialize(serializer),
        }
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Presence<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(deserializer)?;
        if v.is_null() {
            Ok(Self::Null)
        } else {
            T::deserialize(v)
                .map(Self::Value)
                .map_err(de::Error::custom)
        }
    }
}

/// JSON-RPC identifiers retain their exact JSON number representation.
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
    #[serde(default, skip_serializing_if = "Presence::is_absent")]
    pub data: Presence<Value>,
    #[serde(flatten)]
    pub extra: Extras,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: RequestId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Presence::is_absent")]
    pub params: Presence<Value>,
    #[serde(flatten)]
    pub extra: Extras,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Presence::is_absent")]
    pub params: Presence<Value>,
    #[serde(flatten)]
    pub extra: Extras,
}

/// A response has exactly one of `result` and `error`; `result: null` is a
/// successful response and is represented by `Presence::Null`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: RequestId,
    #[serde(default, skip_serializing_if = "Presence::is_absent")]
    pub result: Presence<Value>,
    #[serde(default, skip_serializing_if = "Presence::is_absent")]
    pub error: Presence<RpcError>,
    #[serde(flatten)]
    pub extra: Extras,
}

impl<'de> Deserialize<'de> for Response {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            jsonrpc: String,
            id: RequestId,
            #[serde(default)]
            result: Presence<Value>,
            #[serde(default)]
            error: Presence<RpcError>,
            #[serde(flatten)]
            extra: Extras,
        }
        let wire = Wire::deserialize(deserializer)?;
        if wire.jsonrpc != "2.0" {
            return Err(de::Error::custom("JSON-RPC version must be 2.0"));
        }
        if matches!(wire.result, Presence::Absent) == matches!(wire.error, Presence::Absent) {
            return Err(de::Error::custom(
                "response must contain exactly one of result or error",
            ));
        }
        if matches!(wire.error, Presence::Null) {
            return Err(de::Error::custom("response error must be an object"));
        }
        Ok(Self {
            jsonrpc: wire.jsonrpc,
            id: wire.id,
            result: wire.result,
            error: wire.error,
            extra: wire.extra,
        })
    }
}

/// Envelope classification is based on discriminating members, not enum order.
#[derive(Clone, Debug, PartialEq)]
pub enum Envelope {
    Request(Request),
    Notification(Notification),
    Response(Response),
}
impl Serialize for Envelope {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Request(v) => v.serialize(s),
            Self::Notification(v) => v.serialize(s),
            Self::Response(v) => v.serialize(s),
        }
    }
}
impl<'de> Deserialize<'de> for Envelope {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let o = v
            .as_object()
            .ok_or_else(|| de::Error::custom("JSON-RPC envelope must be an object"))?;
        if o.get("jsonrpc") != Some(&Value::String("2.0".to_owned())) {
            return Err(de::Error::custom("JSON-RPC version must be 2.0"));
        }
        let shape = (
            o.contains_key("method"),
            o.contains_key("id"),
            o.contains_key("result"),
            o.contains_key("error"),
        );
        match shape {
            (true, true, false, false) => serde_json::from_value(v)
                .map(Self::Request)
                .map_err(de::Error::custom),
            (true, false, false, false) => serde_json::from_value(v)
                .map(Self::Notification)
                .map_err(de::Error::custom),
            (false, true, result, error) if result ^ error => serde_json::from_value(v)
                .map(Self::Response)
                .map_err(de::Error::custom),
            _ => Err(de::Error::custom("invalid or ambiguous JSON-RPC envelope")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeanMethod {
    PlainGoal,
    PlainTermGoal,
    RpcConnect,
    RpcCall,
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
            "$/lean/rpc/call" => Self::RpcCall,
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextDocumentIdentifier {
    pub uri: String,
    #[serde(flatten)]
    pub extra: Extras,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextDocumentPositionParams {
    #[serde(rename = "textDocument")]
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
    #[serde(flatten)]
    pub extra: Extras,
}
pub type PlainGoalParams = TextDocumentPositionParams;
pub type PlainTermGoalParams = TextDocumentPositionParams;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlainGoal {
    pub rendered: String,
    pub goals: Vec<String>,
    #[serde(flatten)]
    pub extra: Extras,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlainTermGoal {
    pub goal: String,
    pub range: Range,
    #[serde(flatten)]
    pub extra: Extras,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcConnectParams {
    pub uri: String,
    #[serde(flatten)]
    pub extra: Extras,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcConnected {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(flatten)]
    pub extra: Extras,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcCallParams {
    #[serde(flatten)]
    pub text_document_position: TextDocumentPositionParams,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub method: String,
    pub params: Value,
    #[serde(flatten)]
    pub extra: Extras,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcReleaseParams {
    pub uri: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub refs: Vec<Value>,
    #[serde(flatten)]
    pub extra: Extras,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcKeepAliveParams {
    pub uri: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(flatten)]
    pub extra: Extras,
}

/// Recursive encoding of Lean.Widget.TaggedText α.  Tag values stay JSON
/// because α is caller-defined and may itself contain opaque RPC references.
#[derive(Clone, Debug, PartialEq)]
pub enum TaggedText {
    Text(String),
    Append(Vec<TaggedText>),
    Tag { tag: Value, value: Box<TaggedText> },
    Unknown(Value),
}
impl Serialize for TaggedText {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let v = match self {
            Self::Text(t) => Value::String(t.clone()),
            Self::Append(c) => serde_json::json!({"append": c}),
            Self::Tag { tag, value } => serde_json::json!({"tag": [tag, value]}),
            Self::Unknown(v) => v.clone(),
        };
        v.serialize(s)
    }
}
impl<'de> Deserialize<'de> for TaggedText {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        match &v {
            Value::String(t) => Ok(Self::Text(t.clone())),
            Value::Object(o) if o.len() == 1 && o.contains_key("append") => {
                let a = o["append"]
                    .as_array()
                    .ok_or_else(|| de::Error::custom("TaggedText.append must be an array"))?;
                a.iter()
                    .cloned()
                    .map(serde_json::from_value)
                    .collect::<Result<Vec<_>, _>>()
                    .map(Self::Append)
                    .map_err(de::Error::custom)
            }
            Value::Object(o) if o.len() == 1 && o.contains_key("tag") => {
                let a = o["tag"].as_array().ok_or_else(|| {
                    de::Error::custom("TaggedText.tag must be a two-element array")
                })?;
                if a.len() != 2 {
                    return Err(de::Error::custom("TaggedText.tag must have two elements"));
                }
                let value = serde_json::from_value(a[1].clone()).map_err(de::Error::custom)?;
                Ok(Self::Tag {
                    tag: a[0].clone(),
                    value: Box::new(value),
                })
            }
            _ => Ok(Self::Unknown(v)),
        }
    }
}

/// Lean's opaque `RpcRef`, in either v0 (`p`) or v1 (`__rpcref`) encoding.
/// The id remains JSON to preserve strings and arbitrarily large numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct OpaqueReference {
    pub field: String,
    pub id: Value,
    pub extra: Extras,
}
pub type RpcRef = OpaqueReference;
pub type OpaqueRef = OpaqueReference;
impl Serialize for OpaqueReference {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut o = self.extra.clone();
        o.insert(self.field.clone(), self.id.clone());
        o.serialize(s)
    }
}
impl<'de> Deserialize<'de> for OpaqueReference {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let mut o = v
            .as_object()
            .cloned()
            .ok_or_else(|| de::Error::custom("RPC reference must be an object"))?;
        let field = if o.contains_key("__rpcref") {
            "__rpcref"
        } else if o.contains_key("p") {
            "p"
        } else {
            return Err(de::Error::custom(
                "RPC reference must contain p or __rpcref",
            ));
        };
        let id = o.remove(field).expect("checked above");
        Ok(Self {
            field: field.to_owned(),
            id,
            extra: o.into_iter().collect(),
        })
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RpcWireFormat {
    V0,
    V1,
}
impl RpcWireFormat {
    pub fn ref_field(self) -> &'static str {
        match self {
            Self::V0 => "p",
            Self::V1 => "__rpcref",
        }
    }
}
impl Serialize for RpcWireFormat {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Self::V0 => "v0",
            Self::V1 => "v1",
        })
    }
}
impl<'de> Deserialize<'de> for RpcWireFormat {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match String::deserialize(d)?.as_str() {
            "v0" => Ok(Self::V0),
            "v1" => Ok(Self::V1),
            _ => Err(de::Error::custom("RPC wire format must be v0 or v1")),
        }
    }
}

pub type Opaque = Value;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn response_distinguishes_absent_result_from_null_result() {
        let null: Response =
            serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"result":null})).unwrap();
        assert!(null.result.is_null());
        let error: Response = serde_json::from_value(
            json!({"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"x","data":null}}),
        )
        .unwrap();
        assert!(error.result.is_absent());
        assert!(matches!(
            error.error,
            Presence::Value(RpcError {
                data: Presence::Null,
                ..
            })
        ));

        let absent: Request = serde_json::from_value(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "m"
        }))
        .unwrap();
        let null: Request = serde_json::from_value(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "m",
            "params": null
        }))
        .unwrap();
        assert!(absent.params.is_absent());
        assert!(null.params.is_null());
    }

    #[test]
    fn envelope_classification_rejects_ambiguous_shapes() {
        let n: Envelope = serde_json::from_value(json!({"jsonrpc":"2.0","method":"m"})).unwrap();
        assert!(matches!(n, Envelope::Notification(_)));
        assert!(serde_json::from_value::<Envelope>(
            json!({"jsonrpc":"2.0","id":1,"method":"m","result":null})
        )
        .is_err());
    }

    #[test]
    fn tagged_text_uses_lean_recursive_constructor_encoding() {
        let source = json!({"append":["x",{"tag":[{"kind":"type"},{"append":["y"]}]}]});
        let text: TaggedText = serde_json::from_value(source.clone()).unwrap();
        assert_eq!(serde_json::to_value(text).unwrap(), source);
    }

    #[test]
    fn schema_methods_and_rpc_refs_round_trip() {
        assert_eq!(LeanMethod::classify("$/lean/rpc/call"), LeanMethod::RpcCall);
        let call: RpcCallParams = serde_json::from_value(json!({
            "textDocument": {"uri": "file:///Main.lean"},
            "position": {"line": 2, "character": 3},
            "sessionId": "42",
            "method": "Lean.Widget.inspect",
            "params": {"big": 9007199254740993_i64},
            "future": {"keep": true}
        }))
        .unwrap();
        assert_eq!(call.session_id, "42");
        assert_eq!(call.params["big"], json!(9007199254740993_i64));
        assert_eq!(call.extra["future"], json!({"keep": true}));
        let p: PlainGoal =
            serde_json::from_value(json!({"rendered":"no goals","goals":[]})).unwrap();
        assert_eq!(p.goals, Vec::<String>::new());
        let r: RpcRef =
            serde_json::from_value(json!({"__rpcref":"18446744073709551615","future":true}))
                .unwrap();
        assert_eq!(r.id, json!("18446744073709551615"));
        assert_eq!(
            serde_json::to_value(r).unwrap(),
            json!({"__rpcref":"18446744073709551615","future":true})
        );
    }
}
