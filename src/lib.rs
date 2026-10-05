//! Rust-owned state for the Steel/Helix host integration.
//!
//! The editor and language-server lifetimes remain owned by Helix. Scheme is
//! only the thin adapter that registers Helix hooks and builds LSP values;
//! counters and lifecycle state live here so reloads cannot silently replace
//! the state used by the native component.

use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
};

use serde_json::Value;

use steel::{
    rvals::Custom,
    steel_vm::ffi::{FFIModule, RegisterFFIFn},
};

use crate::{
    correspondence::{first_location, same_document, terminal_spans, TerminalSpan},
    goals::GoalState,
    protocol::{Extras, PlainGoal, Position},
};

const MAX_PANEL_ROWS: usize = 512;
const MAX_INFO_LINES: usize = 64;
const MAX_INFO_LINE_CHARS: usize = 512;

macro_rules! trace {
    ($($arg:tt)*) => {
        if std::env::var_os("LEAN4_HX_TRACE").is_some() {
            eprintln!($($arg)*);
        }
    };
}

pub mod actions;
pub mod adapters;
pub mod correspondence;
pub mod cursor_info;
pub mod goals;
pub mod protocol;
pub mod unicode;

#[derive(Debug)]
struct NativeState {
    active: AtomicBool,
    renders: AtomicUsize,
    installs: AtomicUsize,
    removals: AtomicUsize,
    selections: AtomicUsize,
    inserts: AtomicUsize,
    opened: AtomicUsize,
    closed: AtomicUsize,
    requests: AtomicUsize,
    callbacks: AtomicUsize,
    generation: AtomicUsize,
    document_version: AtomicUsize,
    request_inflight: AtomicBool,
    action_inflight: AtomicBool,
    focused: AtomicBool,
    scroll: AtomicUsize,
    selected_goal: AtomicUsize,
    last_request: Mutex<String>,
    last_callback: Mutex<String>,
    goal: Mutex<String>,
    navigation: Mutex<String>,
    navigation_target: Mutex<Option<crate::protocol::Location>>,
    current_uri: Mutex<String>,
    goal_state: Mutex<GoalState>,
    rpc_session: Mutex<Option<Value>>,
    rpc_refs: Mutex<Vec<Value>>,
    cursor_info: Mutex<CursorInfo>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct CursorInfo {
    stamp: Option<crate::goals::RequestStamp>,
    lines: Vec<String>,
}

impl Custom for NativeState {}

impl NativeState {
    fn decode_payload(raw: &str, context: &str) -> Option<Value> {
        match adapters::TextTraceAdapter::decode(raw) {
            adapters::AdaptedPayload::Structured(value) => {
                trace!("LEAN4_HX_ADAPTER mode=structured context={context}");
                Some(value)
            }
            adapters::AdaptedPayload::Fallback { bytes, truncated } => {
                trace!(
                    "LEAN4_HX_ADAPTER mode=fallback context={context} bytes={bytes} truncated={truncated}"
                );
                None
            }
        }
    }

    fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            renders: AtomicUsize::new(0),
            installs: AtomicUsize::new(0),
            removals: AtomicUsize::new(0),
            selections: AtomicUsize::new(0),
            inserts: AtomicUsize::new(0),
            opened: AtomicUsize::new(0),
            closed: AtomicUsize::new(0),
            requests: AtomicUsize::new(0),
            callbacks: AtomicUsize::new(0),
            generation: AtomicUsize::new(0),
            document_version: AtomicUsize::new(1),
            request_inflight: AtomicBool::new(false),
            action_inflight: AtomicBool::new(false),
            focused: AtomicBool::new(false),
            scroll: AtomicUsize::new(0),
            selected_goal: AtomicUsize::new(0),
            last_request: Mutex::new("none".to_owned()),
            last_callback: Mutex::new("none".to_owned()),
            goal: Mutex::new("Lean goal unavailable".to_owned()),
            navigation: Mutex::new("Navigation unavailable".to_owned()),
            navigation_target: Mutex::new(None),
            current_uri: Mutex::new(String::new()),
            goal_state: Mutex::new(GoalState::default()),
            rpc_session: Mutex::new(None),
            rpc_refs: Mutex::new(Vec::new()),
            cursor_info: Mutex::new(CursorInfo::default()),
        }
    }

    fn clear_cursor_info(&self) {
        *self.cursor_info.lock().expect("cursor info poisoned") = CursorInfo::default();
    }

    fn clear_rpc_session(&self) {
        *self.rpc_session.lock().expect("rpc state poisoned") = None;
        self.rpc_refs.lock().expect("rpc state poisoned").clear();
    }

    fn reset_rpc_after_server_restart(&self) {
        if !self.is_active() {
            return;
        }
        // Helix may report the restart command more than once. Once the first
        // callback has released the session and invalidated pending work, a
        // duplicate notification must not advance generations again.
        let has_session = self
            .rpc_session
            .lock()
            .expect("rpc state poisoned")
            .is_some();
        let has_pending = self.request_inflight.load(Ordering::Acquire)
            || self.action_inflight.load(Ordering::Acquire);
        if !has_session && !has_pending {
            return;
        }
        self.clear_rpc_session();
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.request_inflight.store(false, Ordering::Release);
        self.action_inflight.store(false, Ordering::Release);
        self.scroll.store(0, Ordering::Release);
        self.selected_goal.store(0, Ordering::Release);
        self.goal_state
            .lock()
            .expect("goal state poisoned")
            .invalidate_and_advance("server restarting");
        self.clear_cursor_info();
        trace!("LEAN4_HX_RPC action=reset");
    }

    fn activate(&self) {
        self.active.store(true, Ordering::Release);
        self.installs.fetch_add(1, Ordering::Relaxed);
        trace!("LEAN4_HX_INSTALL active=true");
    }

    fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
        self.clear_rpc_session();
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.request_inflight.store(false, Ordering::Release);
        self.action_inflight.store(false, Ordering::Release);
        self.focused.store(false, Ordering::Release);
        self.scroll.store(0, Ordering::Release);
        self.selected_goal.store(0, Ordering::Release);
        self.goal_state.lock().expect("goal state poisoned").close();
        self.clear_cursor_info();
        *self.goal.lock().expect("goal state poisoned") = "Lean goal unavailable".to_owned();
        *self.navigation.lock().expect("navigation state poisoned") =
            "Navigation unavailable".to_owned();
        *self
            .navigation_target
            .lock()
            .expect("navigation state poisoned") = None;
        self.removals.fetch_add(1, Ordering::Relaxed);
        trace!("LEAN4_HX_REMOVE active=false");
    }

    fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    fn record_selection(&self) {
        if self.is_active() {
            self.selections.fetch_add(1, Ordering::Relaxed);
            trace!("LEAN4_HX_SELECTION");
        }
    }

    fn record_insert(&self) {
        if self.is_active() {
            self.record_document_change();
        }
    }

    fn record_document_change(&self) {
        if !self.is_active() {
            return;
        }
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.document_version.fetch_add(1, Ordering::AcqRel);
        self.request_inflight.store(false, Ordering::Release);
        self.action_inflight.store(false, Ordering::Release);
        self.scroll.store(0, Ordering::Release);
        self.selected_goal.store(0, Ordering::Release);
        self.goal_state
            .lock()
            .expect("goal state poisoned")
            .invalidate_and_advance("document changed");
        self.clear_cursor_info();
        *self.goal.lock().expect("goal state poisoned") = "Lean goal unavailable".to_owned();
        *self.navigation.lock().expect("navigation state poisoned") =
            "Navigation unavailable".to_owned();
        *self
            .navigation_target
            .lock()
            .expect("navigation state poisoned") = None;
        self.inserts.fetch_add(1, Ordering::Relaxed);
        trace!(
            "LEAN4_HX_DOCUMENT_CHANGED version={}",
            self.document_version.load(Ordering::Acquire)
        );
    }

    fn record_opened(&self) {
        if self.is_active() {
            self.clear_rpc_session();
            self.generation.fetch_add(1, Ordering::AcqRel);
            self.document_version.store(1, Ordering::Release);
            self.request_inflight.store(false, Ordering::Release);
            self.action_inflight.store(false, Ordering::Release);
            self.scroll.store(0, Ordering::Release);
            self.selected_goal.store(0, Ordering::Release);
            self.goal_state.lock().expect("goal state poisoned").close();
            self.clear_cursor_info();
            *self.goal.lock().expect("goal state poisoned") = "Lean goal unavailable".to_owned();
            *self.navigation.lock().expect("navigation state poisoned") =
                "Navigation unavailable".to_owned();
            *self
                .navigation_target
                .lock()
                .expect("navigation state poisoned") = None;
            self.opened.fetch_add(1, Ordering::Relaxed);
            trace!("LEAN4_HX_OPEN");
        }
    }

    fn record_closed(&self) {
        if self.is_active() {
            self.clear_rpc_session();
            self.generation.fetch_add(1, Ordering::AcqRel);
            self.request_inflight.store(false, Ordering::Release);
            self.action_inflight.store(false, Ordering::Release);
            self.focused.store(false, Ordering::Release);
            self.scroll.store(0, Ordering::Release);
            self.selected_goal.store(0, Ordering::Release);
            self.goal_state.lock().expect("goal state poisoned").close();
            self.clear_cursor_info();
            *self.goal.lock().expect("goal state poisoned") = "Lean goal unavailable".to_owned();
            *self.navigation.lock().expect("navigation state poisoned") =
                "Navigation unavailable".to_owned();
            *self
                .navigation_target
                .lock()
                .expect("navigation state poisoned") = None;
            self.closed.fetch_add(1, Ordering::Relaxed);
            trace!("LEAN4_HX_CLOSE");
        }
    }

    fn begin_request(&self, path: String, line: usize, character: usize) -> usize {
        if !self.is_active()
            || self
                .request_inflight
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return 0;
        }
        let position = Position {
            line: line as u32,
            character: character as u32,
            extra: Extras::new(),
        };
        let uri = file_uri(&path);
        *self.current_uri.lock().expect("navigation state poisoned") = uri.clone();
        let stamp = self.goal_state.lock().expect("goal state poisoned").begin(
            uri,
            self.document_version.load(Ordering::Acquire) as i32,
            position,
        );
        self.generation
            .store(stamp.generation as usize, Ordering::Release);
        stamp.generation as usize
    }

    fn cancel_request(&self) {
        *self
            .navigation_target
            .lock()
            .expect("navigation state poisoned") = None;
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.request_inflight.store(false, Ordering::Release);
        self.action_inflight.store(false, Ordering::Release);
        self.scroll.store(0, Ordering::Release);
        self.goal_state
            .lock()
            .expect("goal state poisoned")
            .invalidate_and_advance("selection changed");
        self.clear_cursor_info();
        *self.navigation.lock().expect("navigation state poisoned") =
            "Navigation unavailable".to_owned();
    }

    fn record_request(&self, _path: String, line: usize, character: usize) {
        if self.is_active() {
            self.requests.fetch_add(1, Ordering::Relaxed);
            let mut last = self.last_request.lock().expect("request state poisoned");
            *last = format!("line={line},character={character}");
            trace!("LEAN4_HX_REQUEST line={line} character={character}");
        }
    }

    fn record_callback(&self, generation: usize, result: String) {
        self.record_callback_with_completion(generation, result, true);
    }

    fn record_callback_with_completion(
        &self,
        generation: usize,
        result: String,
        allow_completion: bool,
    ) {
        if self.is_active() && self.generation.load(Ordering::Acquire) == generation {
            self.callbacks.fetch_add(1, Ordering::Relaxed);
            let mut last = self.last_callback.lock().expect("callback state poisoned");
            *last = "reply".to_owned();
            let goal = if result == "null" {
                let mut state = self.goal_state.lock().expect("goal state poisoned");
                state.accept_no_goal_with_completion(
                    generation as u64,
                    "Lean returned no goal",
                    allow_completion,
                );
                state.display_text()
            } else {
                let Some(value) = Self::decode_payload(&result, "plain-goal") else {
                    let mut state = self.goal_state.lock().expect("goal state poisoned");
                    state.accept_unavailable(generation as u64, "malformed host callback");
                    *self.goal.lock().expect("goal state poisoned") = state.display_text();
                    self.request_inflight.store(false, Ordering::Release);
                    trace!("LEAN4_HX_CALLBACK result=fallback");
                    return;
                };
                let accepted = Self::plain_goal(&value)
                    .map(|plain| {
                        self.goal_state
                            .lock()
                            .expect("goal state poisoned")
                            .accept_plain_goal_with_completion(
                                generation as u64,
                                plain,
                                allow_completion,
                            )
                    })
                    .unwrap_or(false);
                if !accepted {
                    trace!("LEAN4_HX_CALLBACK result=stale");
                    return;
                }
                self.goal_state
                    .lock()
                    .expect("goal state poisoned")
                    .display_text()
            };
            self.request_inflight.store(false, Ordering::Release);
            self.scroll.store(0, Ordering::Release);
            self.selected_goal.store(0, Ordering::Release);
            *self.goal.lock().expect("goal state poisoned") = goal.clone();
            trace!("LEAN4_HX_CALLBACK result=reply");
            trace!(
                "LEAN4_HX_GOAL generation={generation} available={}",
                goal != "Lean goal unavailable" && !goal.is_empty()
            );
            if goal != "Lean goal unavailable" && !goal.is_empty() {
                let lines = self
                    .goal_state
                    .lock()
                    .expect("goal state poisoned")
                    .snapshot()
                    .map(|snapshot| snapshot.display_lines().len())
                    .unwrap_or(0);
                trace!("LEAN4_HX_GOAL_RENDER_READY lines={lines} available=true");
                trace!("LEAN4_HX_GOAL_AVAILABLE");
            }
        } else {
            trace!("LEAN4_HX_CALLBACK result=stale");
        }
    }

    fn info_stamp_current(&self, generation: usize) -> Option<crate::goals::RequestStamp> {
        if !self.is_active() || !self.generation_current(generation) {
            return None;
        }
        self.goal_state
            .lock()
            .expect("goal state poisoned")
            .active_stamp()
            .filter(|stamp| stamp.generation == generation as u64)
            .cloned()
    }

    fn record_info(&self, generation: usize, result: String, kind: &str) {
        let Some(stamp) = self.info_stamp_current(generation) else {
            trace!("LEAN4_HX_INFO kind={kind} result=stale");
            return;
        };
        let Some(value) = Self::decode_payload(&result, kind) else {
            self.clear_cursor_info();
            trace!("LEAN4_HX_INFO kind={kind} result=malformed");
            return;
        };
        let Some(lines) = crate::cursor_info::parse(kind, &value, &stamp.position) else {
            trace!("LEAN4_HX_INFO kind={kind} result=empty");
            return;
        };
        if lines.is_empty() {
            trace!("LEAN4_HX_INFO kind={kind} result=empty");
            return;
        }
        let mut info = self.cursor_info.lock().expect("cursor info poisoned");
        if info.stamp.as_ref() != Some(&stamp) {
            info.stamp = Some(stamp);
            info.lines.clear();
        }
        for line in lines {
            if !info.lines.contains(&line) && info.lines.len() < MAX_INFO_LINES {
                info.lines
                    .push(line.chars().take(MAX_INFO_LINE_CHARS).collect());
            }
        }
        trace!("LEAN4_HX_INFO kind={kind} result=ready lines={}", info.lines.len());
    }

    fn record_hover(&self, generation: usize, result: String) {
        self.record_info(generation, result, "hover");
    }

    fn record_signature(&self, generation: usize, result: String) {
        self.record_info(generation, result, "signature");
    }

    fn record_inlay(&self, generation: usize, result: String) {
        self.record_info(generation, result, "inlay");
    }

    fn record_navigation(&self, generation: usize, result: String) {
        if !self.is_active() || self.generation.load(Ordering::Acquire) != generation {
            trace!("LEAN4_HX_NAVIGATION stale");
            return;
        }
        let current_uri = self
            .current_uri
            .lock()
            .expect("navigation state poisoned")
            .clone();
        *self
            .navigation_target
            .lock()
            .expect("navigation state poisoned") = None;
        let navigation = Self::decode_payload(&result, "navigation")
            .and_then(|value| first_location(&value))
            .map(|location| {
                *self
                    .navigation_target
                    .lock()
                    .expect("navigation state poisoned") = Some(location.clone());
                let relation = if same_document(&current_uri, &location) {
                    "same-file"
                } else {
                    "cross-file"
                };
                trace!("LEAN4_HX_NAVIGATION {relation}");
                format!("navigation {relation}")
            })
            .unwrap_or_else(|| {
                trace!("LEAN4_HX_NAVIGATION unavailable");
                "navigation unavailable".to_owned()
            });
        *self.navigation.lock().expect("navigation state poisoned") = navigation;
    }

    fn record_navigation_applied(&self, path: String, line: usize, character: usize) {
        if self.is_active() {
            trace!(
                "LEAN4_HX_NAVIGATION_APPLIED path={path} line={line} character={character}"
            );
        }
    }

    fn navigation_uri(&self) -> String {
        self.navigation_target
            .lock()
            .expect("navigation state poisoned")
            .as_ref()
            .map(|location| location.uri.clone())
            .unwrap_or_default()
    }

    fn navigation_path(&self) -> String {
        local_file_path(&self.navigation_uri()).unwrap_or_default()
    }

    fn navigation_target_json(&self) -> String {
        let target = self
            .navigation_target
            .lock()
            .expect("navigation state poisoned");
        let Some(location) = target.as_ref() else {
            return "{}".to_owned();
        };
        let Some(path) = local_file_path(&location.uri) else {
            return "{}".to_owned();
        };
        serde_json::json!({
            "path": path,
            "start-line": location.range.start.line,
            "start-character": location.range.start.character,
            "end-line": location.range.end.line,
            "end-character": location.range.end.character,
        })
        .to_string()
    }

    fn navigation_start_line(&self) -> usize {
        self.navigation_target
            .lock()
            .expect("navigation state poisoned")
            .as_ref()
            .map(|location| location.range.start.line as usize)
            .unwrap_or(0)
    }

    fn navigation_start_character(&self) -> usize {
        self.navigation_target
            .lock()
            .expect("navigation state poisoned")
            .as_ref()
            .map(|location| location.range.start.character as usize)
            .unwrap_or(0)
    }

    fn navigation_end_line(&self) -> usize {
        self.navigation_target
            .lock()
            .expect("navigation state poisoned")
            .as_ref()
            .map(|location| location.range.end.line as usize)
            .unwrap_or(0)
    }

    fn navigation_end_character(&self) -> usize {
        self.navigation_target
            .lock()
            .expect("navigation state poisoned")
            .as_ref()
            .map(|location| location.range.end.character as usize)
            .unwrap_or(0)
    }

    fn collect_rpc_refs(value: &Value, refs: &mut Vec<Value>) {
        match value {
            Value::Object(object)
                if object.len() == 1
                    && (object.contains_key("__rpcref") || object.contains_key("p")) =>
            {
                if !refs.iter().any(|existing| existing == value) {
                    refs.push(value.clone());
                }
            }
            Value::Object(object) => object
                .values()
                .for_each(|value| Self::collect_rpc_refs(value, refs)),
            Value::Array(values) => values
                .iter()
                .for_each(|value| Self::collect_rpc_refs(value, refs)),
            _ => {}
        }
    }

    fn record_rpc(&self, generation: usize, result: String) {
        self.record_rpc_with_completion(generation, result, true);
    }

    fn record_rpc_with_completion(
        &self,
        generation: usize,
        result: String,
        allow_completion: bool,
    ) {
        if self.is_active() && self.generation.load(Ordering::Acquire) == generation {
            let Some(value) = Self::decode_payload(&result, "interactive-goals") else {
                self.clear_rpc_session();
                trace!("LEAN4_HX_RPC action=reply");
                return;
            };
            if value.get("error").is_some() {
                self.clear_rpc_session();
                trace!("LEAN4_HX_RPC action=error");
                return;
            }
            if let Some(session) = value.get("sessionId") {
                let mut current = self.rpc_session.lock().expect("rpc state poisoned");
                if current.as_ref() != Some(session) {
                    *current = Some(session.clone());
                    self.rpc_refs.lock().expect("rpc state poisoned").clear();
                }
                trace!("LEAN4_HX_RPC action=connect");
            } else {
                let stamp = self
                    .goal_state
                    .lock()
                    .expect("goal state poisoned")
                    .active_stamp()
                    .filter(|stamp| stamp.generation == generation as u64)
                    .cloned();
                if let Some(stamp) = stamp {
                    if let Some(snapshot) =
                        crate::goals::GoalSnapshot::from_interactive_goals(stamp, &value)
                    {
                        let mut refs = Vec::new();
                        Self::collect_rpc_refs(&value, &mut refs);
                        *self.rpc_refs.lock().expect("rpc state poisoned") = refs;
                        let mut state = self.goal_state.lock().expect("goal state poisoned");
                        let accepted = if !allow_completion && snapshot.completed {
                            state.clear_completion_candidate();
                            state.accept_unavailable(generation as u64, "Lean reported an error")
                        } else if allow_completion && snapshot.completed {
                            state.accept_completed(snapshot)
                        } else {
                            state.accept(snapshot)
                        };
                        if accepted {
                            let text = state.display_text();
                            *self.goal.lock().expect("goal state poisoned") = text.clone();
                            self.request_inflight.store(false, Ordering::Release);
                            self.scroll.store(0, Ordering::Release);
                            self.selected_goal.store(0, Ordering::Release);
                            let lines = state
                                .snapshot()
                                .map(|snapshot| snapshot.display_lines().len())
                                .unwrap_or(0);
                            trace!("LEAN4_HX_RPC action=interactive-goals");
                            trace!("LEAN4_HX_TAGGED_RENDER_READY lines={lines} available=true");
                            return;
                        }
                    }
                }
                trace!("LEAN4_HX_RPC action=reply");
            }
        } else {
            trace!("LEAN4_HX_RPC stale");
        }
    }

    fn rpc_session_current(&self, _generation: usize) -> bool {
        self.is_active()
            && self
                .rpc_session
                .lock()
                .expect("rpc state poisoned")
                .is_some()
    }

    fn rpc_session_goals_request(&self, generation: usize) -> String {
        if !self.generation_current(generation) {
            return "{}".into();
        }
        let session = self.rpc_session.lock().expect("rpc state poisoned").clone();
        let Some(session) = session else {
            return "{}".into();
        };
        let state = self.goal_state.lock().expect("goal state poisoned");
        let Some(stamp) = state
            .active_stamp()
            .filter(|stamp| stamp.generation == generation as u64)
        else {
            return "{}".into();
        };
        let params = serde_json::json!({
            "textDocument": {"uri": stamp.uri},
            "position": stamp.position
        });
        serde_json::json!({
            "textDocument": {"uri": stamp.uri},
            "position": stamp.position,
            "sessionId": session,
            "method": "Lean.Widget.getInteractiveGoals",
            "params": params
        })
        .to_string()
    }

    /// Build the one supported user-triggered RPC action. The method is an
    /// explicit capability boundary; callers cannot supply an arbitrary Lean
    /// server method or reuse a session after its generation changed.
    fn rpc_term_goal_request(&self, generation: usize) -> String {
        if !self.generation_current(generation)
            || self
                .action_inflight
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return "{}".into();
        }
        let session = self.rpc_session.lock().expect("rpc state poisoned").clone();
        let Some(session) = session else {
            self.action_inflight.store(false, Ordering::Release);
            return "{}".into();
        };
        let state = self.goal_state.lock().expect("goal state poisoned");
        let Some(stamp) = state
            .active_stamp()
            .filter(|stamp| stamp.generation == generation as u64)
            .cloned()
        else {
            self.action_inflight.store(false, Ordering::Release);
            return "{}".into();
        };
        let action = actions::RpcAction {
            uri: stamp.uri.clone(),
            version: stamp.version,
            generation: stamp.generation,
            session_id: session.to_string(),
            method: "Lean.Widget.getInteractiveTermGoal".to_owned(),
            params: serde_json::json!({
                "textDocument": {"uri": stamp.uri},
                "position": stamp.position
            }),
        };
        if let Err(error) = actions::validate_rpc_action(&action, &stamp) {
            self.action_inflight.store(false, Ordering::Release);
            trace!("LEAN4_HX_RPC action=term-goal rejected capability={error:?}");
            return "{}".into();
        }
        let params = serde_json::json!({
            "textDocument": {"uri": stamp.uri},
            "position": stamp.position
        });
        trace!("LEAN4_HX_RPC action=request-term-goal capability=validated");
        serde_json::json!({
            "textDocument": {"uri": stamp.uri},
            "position": stamp.position,
            "sessionId": session,
            "method": "Lean.Widget.getInteractiveTermGoal",
            "params": params
        })
        .to_string()
    }

    fn record_rpc_action(&self, generation: usize, result: String) {
        let Some(stamp) = self.info_stamp_current(generation) else {
            self.action_inflight.store(false, Ordering::Release);
            trace!("LEAN4_HX_RPC action=term-goal stale");
            return;
        };
        let Some(value) = Self::decode_payload(&result, "term-goal") else {
            self.action_inflight.store(false, Ordering::Release);
            self.clear_cursor_info();
            trace!("LEAN4_HX_RPC action=term-goal malformed");
            return;
        };
        if value.get("error").is_some() {
            self.action_inflight.store(false, Ordering::Release);
            trace!("LEAN4_HX_RPC action=term-goal rejected");
            return;
        }
        let value = value.get("result").unwrap_or(&value);
        let Some(tagged_type) = value
            .get("type")
            .and_then(|value| serde_json::from_value::<crate::protocol::TaggedText>(value.clone()).ok())
        else {
            self.action_inflight.store(false, Ordering::Release);
            trace!("LEAN4_HX_RPC action=term-goal unavailable");
            return;
        };
        let mut lines = Vec::new();
        let mut current = String::new();
        for span in terminal_spans(&tagged_type) {
            for part in span.text.split_inclusive('\n') {
                let has_newline = part.ends_with('\n');
                current.push_str(part.trim_end_matches('\n'));
                if has_newline {
                    if !current.trim().is_empty() {
                        lines.push(std::mem::take(&mut current));
                    } else {
                        current.clear();
                    }
                }
            }
        }
        if !current.trim().is_empty() {
            lines.push(current);
        }
        if lines.is_empty() {
            self.action_inflight.store(false, Ordering::Release);
            trace!("LEAN4_HX_RPC action=term-goal unavailable");
            return;
        }
        let mut refs = Vec::new();
        Self::collect_rpc_refs(value, &mut refs);
        if !refs.is_empty() {
            let mut owned = self.rpc_refs.lock().expect("rpc state poisoned");
            for reference in refs {
                if !owned.iter().any(|existing| existing == &reference) {
                    owned.push(reference);
                }
            }
        }
        let mut info = self.cursor_info.lock().expect("cursor info poisoned");
        info.stamp = Some(stamp);
        info.lines = lines;
        self.action_inflight.store(false, Ordering::Release);
        trace!("LEAN4_HX_RPC action=term-goal result=ready lines={}", info.lines.len());
    }

    fn rpc_keepalive_request(&self) -> String {
        let session = self.rpc_session.lock().expect("rpc state poisoned").clone();
        let Some(session) = session else {
            return "{}".into();
        };
        let uri = self
            .current_uri
            .lock()
            .expect("navigation state poisoned")
            .clone();
        trace!("LEAN4_HX_RPC action=keepAlive");
        serde_json::json!({"uri": uri, "sessionId": session}).to_string()
    }

    fn rpc_release_request(&self) -> String {
        let session = self.rpc_session.lock().expect("rpc state poisoned").clone();
        let Some(session) = session else {
            return "{}".into();
        };
        let uri = self
            .current_uri
            .lock()
            .expect("navigation state poisoned")
            .clone();
        let refs = self.rpc_refs.lock().expect("rpc state poisoned").clone();
        trace!("LEAN4_HX_RPC action=release refs={} uri={}", refs.len(), uri);
        self.clear_rpc_session();
        serde_json::json!({"uri": uri, "sessionId": session, "refs": refs}).to_string()
    }

    fn plain_goal(value: &Value) -> Option<PlainGoal> {
        if let Some(result) = value.get("result") {
            return Self::plain_goal(result);
        }
        serde_json::from_value(value.clone()).ok()
    }

    fn label(&self) -> String {
        let renders = self.renders.fetch_add(1, Ordering::Relaxed) + 1;
        let goal = self.goal.lock().expect("goal state poisoned").clone();
        let navigation = self
            .navigation
            .lock()
            .expect("navigation state poisoned")
            .clone();
        trace!(
            "LEAN4_HX_RENDER available={}",
            goal != "Lean goal unavailable" && !goal.is_empty()
        );
        format!("lean4.hx goal {renders}: {goal} [{navigation}]")
    }

    fn lines(&self) -> Vec<String> {
        let snapshot = self
            .goal_state
            .lock()
            .expect("goal state poisoned")
            .snapshot()
            .cloned();
        let Some(snapshot) = snapshot else {
            return Vec::new();
        };
        if snapshot.unavailable.is_some() {
            return Vec::new();
        }
        let selected = self
            .selected_goal
            .load(Ordering::Acquire)
            .min(snapshot.goals.len().saturating_sub(1));
        let body = if !snapshot.goals.is_empty() {
            snapshot.goals[selected]
                .lines()
                .map(str::to_owned)
                .collect()
        } else {
            snapshot.display_lines()
        };
        let offset = self
            .scroll
            .load(Ordering::Acquire)
            .min(body.len().saturating_sub(1));
        self.scroll.store(offset, Ordering::Release);
        body.into_iter()
            .skip(offset)
            .take(MAX_PANEL_ROWS)
            .collect()
    }

    fn styled_lines(&self, width: usize) -> String {
        let snapshot = self
            .goal_state
            .lock()
            .expect("goal state poisoned")
            .snapshot()
            .cloned();
        let body = snapshot
            .as_ref()
            .filter(|snapshot| {
                snapshot.unavailable.is_none()
                    && (snapshot.completed || !snapshot.styled_goals.is_empty())
            })
            .map(|snapshot| {
                let selected = self.selected_goal.load(Ordering::Acquire);
                snapshot.styled_lines(selected)
            })
            .unwrap_or_default();
        if body.is_empty() {
            return "[]".into();
        }
        let lines = crate::correspondence::layout_lines(&body, width);
        let offset = self
            .scroll
            .load(Ordering::Acquire)
            .min(lines.len().saturating_sub(1));
        self.scroll.store(offset, Ordering::Release);
        serde_json::to_string(
            &lines
                .into_iter()
                .skip(offset)
                .take(MAX_PANEL_ROWS)
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    /// Return the plain text represented by a wrapped panel-row range.  The
    /// compositor owns drawing, but copying must use the same terminal-cell
    /// layout so a soft wrap never becomes a spurious document newline.
    fn selected_text(&self, width: usize, start: usize, end: usize) -> String {
        let snapshot = self
            .goal_state
            .lock()
            .expect("goal state poisoned")
            .snapshot()
            .cloned();
        let Some(snapshot) = snapshot else {
            return String::new();
        };
        if snapshot.unavailable.is_some() {
            return String::new();
        }
        let selected = self.selected_goal.load(Ordering::Acquire);
        let rows = crate::correspondence::layout_lines(&snapshot.styled_lines(selected), width);
        let lo = start.min(end).min(rows.len());
        let hi = start.max(end).min(rows.len().saturating_sub(1));
        if rows.is_empty() || lo > hi {
            return String::new();
        }
        rows[lo..=hi]
            .iter()
            .map(|row| {
                row.iter()
                    .filter_map(|span| span.get("text").and_then(Value::as_str))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn selected_info_text(&self, width: usize, start: usize, end: usize) -> String {
        let info = self.cursor_info.lock().expect("cursor info poisoned").clone();
        let body = info
            .lines
            .into_iter()
            .map(|text| {
                vec![TerminalSpan {
                    text,
                    style: crate::correspondence::TerminalStyle::Plain,
                    tags: Vec::new(),
                }]
            })
            .collect::<Vec<_>>();
        let rows = crate::correspondence::layout_lines(&body, width);
        let lo = start.min(end).min(rows.len());
        let hi = start.max(end).min(rows.len().saturating_sub(1));
        if rows.is_empty() || lo > hi {
            return String::new();
        }
        rows[lo..=hi]
            .iter()
            .map(|row| {
                row.iter()
                    .filter_map(|span| span.get("text").and_then(Value::as_str))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn goal_line_count(&self, width: usize) -> usize {
        let snapshot = self
            .goal_state
            .lock()
            .expect("goal state poisoned")
            .snapshot()
            .cloned();
        snapshot
            .as_ref()
            .filter(|snapshot| {
                snapshot.unavailable.is_none()
                    && (snapshot.completed || !snapshot.styled_goals.is_empty())
            })
            .map(|snapshot| {
                let selected = self.selected_goal.load(Ordering::Acquire);
                crate::correspondence::layout_lines(&snapshot.styled_lines(selected), width)
                    .len()
                    .min(MAX_PANEL_ROWS)
            })
            .unwrap_or(0)
    }

    fn info_lines(&self, width: usize) -> String {
        let info = self.cursor_info.lock().expect("cursor info poisoned").clone();
        if info.lines.is_empty() {
            return "[]".into();
        }
        let body = info
            .lines
            .into_iter()
            .map(|text| {
                vec![TerminalSpan {
                    text,
                    style: crate::correspondence::TerminalStyle::Plain,
                    tags: Vec::new(),
                }]
            })
            .collect::<Vec<_>>();
        serde_json::to_string(
            &crate::correspondence::layout_lines(&body, width)
                .into_iter()
                .take(MAX_INFO_LINES)
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    fn generation_current(&self, generation: usize) -> bool {
        self.is_active() && self.generation.load(Ordering::Acquire) == generation
    }

    fn generation_number(&self) -> usize {
        self.generation.load(Ordering::Acquire)
    }

    fn rpc_goals_request(&self, generation: usize, result: String) -> String {
        if !self.generation_current(generation) {
            return "{}".into();
        }
        let Some(value) = Self::decode_payload(&result, "rpc-connect") else {
            return "{}".into();
        };
        let Some(session) = value.get("sessionId").cloned() else {
            return "{}".into();
        };
        let state = self.goal_state.lock().expect("goal state poisoned");
        let Some(stamp) = state
            .active_stamp()
            .filter(|s| s.generation == generation as u64)
        else {
            return "{}".into();
        };
        let params =
            serde_json::json!({"textDocument":{"uri":stamp.uri},"position":stamp.position});
        let request =
            serde_json::json!({"textDocument":{"uri":stamp.uri},"position":stamp.position,
            "sessionId":session,"method":"Lean.Widget.getInteractiveGoals","params":params})
            .to_string();
        trace!(
            "LEAN4_HX_RPC_REQUEST generation={generation} method=Lean.Widget.getInteractiveGoals"
        );
        request
    }

    fn focused(&self) -> bool {
        self.focused.load(Ordering::Acquire)
    }

    fn set_focused(&self, focused: bool) {
        self.focused.store(focused, Ordering::Release);
        trace!("LEAN4_HX_GOAL_FOCUS focused={focused}");
    }

    fn scroll(&self, amount: isize) {
        let current = self.scroll.load(Ordering::Acquire) as isize;
        self.scroll.store(
            current.saturating_add(amount).max(0) as usize,
            Ordering::Release,
        );
    }

    fn next_goal(&self) {
        self.scroll.store(0, Ordering::Release);
        let count = self
            .goal_state
            .lock()
            .expect("goal state poisoned")
            .snapshot()
            .map_or(0, |snapshot| snapshot.goals.len());
        if count > 0 {
            self.selected_goal
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |index| {
                    Some((index + 1).min(count - 1))
                })
                .ok();
        }
        trace!(
            "LEAN4_HX_GOAL_SELECTED index={}",
            self.selected_goal.load(Ordering::Acquire)
        );
    }

    fn previous_goal(&self) {
        self.scroll.store(0, Ordering::Release);
        self.selected_goal
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |index| {
                Some(index.saturating_sub(1))
            })
            .ok();
        trace!(
            "LEAN4_HX_GOAL_SELECTED index={}",
            self.selected_goal.load(Ordering::Acquire)
        );
    }

    fn summary(&self) -> String {
        let request = self.last_request.lock().expect("request state poisoned");
        let callback = self.last_callback.lock().expect("callback state poisoned");
        format!(
            "active={} installs={} removals={} renders={} selection={} insert={} open={} close={} requests={} callbacks={} last_request={} last_callback={}",
            self.is_active(),
            self.installs.load(Ordering::Relaxed),
            self.removals.load(Ordering::Relaxed),
            self.renders.load(Ordering::Relaxed),
            self.selections.load(Ordering::Relaxed),
            self.inserts.load(Ordering::Relaxed),
            self.opened.load(Ordering::Relaxed),
            self.closed.load(Ordering::Relaxed),
            self.requests.load(Ordering::Relaxed),
            self.callbacks.load(Ordering::Relaxed),
            &*request,
            &*callback,
        )
    }

    fn unicode_start(&self, line: String, cursor_utf16: usize) -> usize {
        unicode::edit_at_cursor(&line, cursor_utf16)
            .map(|edit| edit.start_chars)
            .unwrap_or(0)
    }

    fn unicode_end(&self, line: String, cursor_utf16: usize) -> usize {
        unicode::edit_at_cursor(&line, cursor_utf16)
            .map(|edit| edit.end_chars)
            .unwrap_or(0)
    }

    fn unicode_replacement(&self, line: String, cursor_utf16: usize) -> String {
        unicode::edit_at_cursor(&line, cursor_utf16)
            .map(|edit| edit.replacement)
            .unwrap_or_default()
    }

    fn unicode_cursor(&self, line: String, cursor_utf16: usize) -> usize {
        unicode::edit_at_cursor(&line, cursor_utf16)
            .map(|edit| edit.cursor_chars)
            .unwrap_or(0)
    }

    fn utf16_to_chars(&self, line: String, cursor_utf16: usize) -> usize {
        unicode::utf16_to_chars(&line, cursor_utf16).unwrap_or(0)
    }
}

/// Decode local file URIs without interpreting shell syntax or accepting remote hosts.
fn local_file_path(uri: &str) -> Option<String> {
    let path = uri.strip_prefix("file://")?;
    let path = path
        .strip_prefix("localhost/")
        .map(|p| format!("/{p}"))
        .unwrap_or_else(|| path.to_owned());
    if !path.starts_with('/') || path.contains(['?', '#']) {
        return None;
    }
    let mut bytes = Vec::new();
    let mut input = path.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = (input.next()? as char).to_digit(16)?;
            let low = (input.next()? as char).to_digit(16)?;
            bytes.push((high * 16 + low) as u8);
        } else {
            bytes.push(byte);
        }
    }
    if bytes.contains(&0) {
        return None;
    }
    let path = String::from_utf8(bytes).ok()?;
    // Avoid Helix's open command creating a scratch buffer for a missing target.
    Path::new(&path).is_file().then_some(path)
}

/// Convert an absolute editor path into the URI required by Lean's LSP.
/// Percent-encoding is implemented locally so the host adapter does not add
/// another dependency solely for this boundary conversion.
fn file_uri(path: &str) -> String {
    let absolute = if Path::new(path).is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path).to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_owned())
    };
    let mut uri = String::from("file://");
    for byte in absolute.as_bytes() {
        let safe = matches!(byte,
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' |
            b'-' | b'.' | b'_' | b'~' | b'/');
        if safe {
            uri.push(*byte as char);
        } else {
            uri.push('%');
            uri.push(
                char::from_digit((byte >> 4) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
            uri.push(
                char::from_digit((byte & 0x0f) as u32, 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
        }
    }
    uri
}

/// Construct the Steel FFI module loaded by `#%require-dylib`.
pub fn build_module() -> FFIModule {
    let mut module = FFIModule::new("dylib/lean4-hx");
    module
        .register_fn("native-state", NativeState::new)
        .register_fn("native-activate!", NativeState::activate)
        .register_fn("native-deactivate!", NativeState::deactivate)
        .register_fn("native-active?", NativeState::is_active)
        .register_fn("native-record-selection!", NativeState::record_selection)
        .register_fn("native-record-insert!", NativeState::record_insert)
        .register_fn(
            "native-record-document-change!",
            NativeState::record_document_change,
        )
        .register_fn("native-record-open!", NativeState::record_opened)
        .register_fn("native-record-close!", NativeState::record_closed)
        .register_fn(
            "native-reset-rpc-after-server-restart!",
            NativeState::reset_rpc_after_server_restart,
        )
        .register_fn("native-record-request!", NativeState::record_request)
        .register_fn("native-begin-request!", NativeState::begin_request)
        .register_fn("native-cancel-request!", NativeState::cancel_request)
        .register_fn("native-record-callback!", NativeState::record_callback)
        .register_fn(
            "native-record-callback-with-completion!",
            NativeState::record_callback_with_completion,
        )
        .register_fn("native-record-hover!", NativeState::record_hover)
        .register_fn("native-record-signature!", NativeState::record_signature)
        .register_fn("native-record-inlay!", NativeState::record_inlay)
        .register_fn("native-record-navigation!", NativeState::record_navigation)
        .register_fn(
            "native-record-navigation-applied!",
            NativeState::record_navigation_applied,
        )
        .register_fn("native-record-rpc!", NativeState::record_rpc)
        .register_fn(
            "native-record-rpc-with-completion!",
            NativeState::record_rpc_with_completion,
        )
        .register_fn("native-record-rpc-action!", NativeState::record_rpc_action)
        .register_fn("native-label", NativeState::label)
        .register_fn("native-lines", NativeState::lines)
        .register_fn("native-styled-lines", NativeState::styled_lines)
        .register_fn("native-selected-text", NativeState::selected_text)
        .register_fn("native-selected-info-text", NativeState::selected_info_text)
        .register_fn("native-goal-line-count", NativeState::goal_line_count)
        .register_fn("native-info-lines", NativeState::info_lines)
        .register_fn("native-rpc-goals-request", NativeState::rpc_goals_request)
        .register_fn(
            "native-rpc-session-current?",
            NativeState::rpc_session_current,
        )
        .register_fn(
            "native-rpc-session-goals-request",
            NativeState::rpc_session_goals_request,
        )
        .register_fn(
            "native-rpc-term-goal-request",
            NativeState::rpc_term_goal_request,
        )
        .register_fn(
            "native-rpc-keepalive-request",
            NativeState::rpc_keepalive_request,
        )
        .register_fn(
            "native-rpc-release-request",
            NativeState::rpc_release_request,
        )
        .register_fn(
            "native-generation-current?",
            NativeState::generation_current,
        )
        .register_fn("native-generation", NativeState::generation_number)
        .register_fn("native-focused?", NativeState::focused)
        .register_fn("native-set-focused!", NativeState::set_focused)
        .register_fn("native-scroll!", NativeState::scroll)
        .register_fn("native-next-goal!", NativeState::next_goal)
        .register_fn("native-previous-goal!", NativeState::previous_goal)
        .register_fn("native-summary", NativeState::summary)
        .register_fn("native-unicode-start", NativeState::unicode_start)
        .register_fn("native-unicode-end", NativeState::unicode_end)
        .register_fn(
            "native-unicode-replacement",
            NativeState::unicode_replacement,
        )
        .register_fn("native-unicode-cursor", NativeState::unicode_cursor)
        .register_fn("native-utf16-to-chars", NativeState::utf16_to_chars)
        .register_fn(
            "native-navigation-target",
            NativeState::navigation_target_json,
        )
        .register_fn("native-navigation-path", NativeState::navigation_path)
        .register_fn("native-navigation-uri", NativeState::navigation_uri)
        .register_fn(
            "native-navigation-start-line",
            NativeState::navigation_start_line,
        )
        .register_fn(
            "native-navigation-start-character",
            NativeState::navigation_start_character,
        )
        .register_fn(
            "native-navigation-end-line",
            NativeState::navigation_end_line,
        )
        .register_fn(
            "native-navigation-end-character",
            NativeState::navigation_end_character,
        )
        .register_fn("native-file-uri", file_uri);
    module
}

steel::declare_module!(build_module);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_state_rejects_stale_goal_after_cancel() {
        let state = NativeState::new();
        state.activate();
        let generation = state.begin_request("/tmp/Main.lean".into(), 0, 3);
        assert_ne!(generation, 0);
        assert_eq!(state.begin_request("/tmp/Main.lean".into(), 1, 0), 0);
        state.cancel_request();
        state.record_callback(
            generation,
            r#"{"goals":["n : Nat\n⊢ n = n"],"rendered":"goal"}"#.into(),
        );
        assert_eq!(
            *state.goal.lock().expect("goal state poisoned"),
            "Lean goal unavailable"
        );
    }

    #[test]
    fn native_panel_is_empty_without_a_current_goal() {
        let state = NativeState::new();
        state.activate();
        assert_eq!(state.styled_lines(36), "[]");

        let generation = state.begin_request("/tmp/Main.lean".into(), 0, 3);
        state.record_callback(generation, "null".into());
        assert_eq!(state.styled_lines(36), "[]");
        assert!(state.lines().is_empty());
    }

    #[test]
    fn malformed_callback_clears_the_previous_goal() {
        let state = NativeState::new();
        state.activate();
        let generation = state.begin_request("/tmp/Main.lean".into(), 0, 0);
        state.record_callback(
            generation,
            r#"{"goals":["n : Nat\n⊢ n = n"]}"#.into(),
        );
        assert!(!state.styled_lines(36).is_empty());
        state.record_callback(generation, "malformed callback".into());
        assert_eq!(state.styled_lines(36), "[]");
    }

    #[test]
    fn panel_copy_uses_wrapped_rows_without_extra_newlines() {
        let state = NativeState::new();
        state.activate();
        let generation = state.begin_request("/tmp/Main.lean".into(), 0, 0);
        state.record_callback(generation, "null".into());
        state.record_hover(
            generation,
            r#"{"contents":"Nat : Type\nThis prose is only for explicit hover","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}}}"#.into(),
        );
        assert_eq!(state.selected_info_text(80, 0, 0), "Nat : Type");
        assert_eq!(state.selected_info_text(80, 1, 1), "");
    }

    #[test]
    fn information_surface_has_a_bounded_line_budget() {
        let state = NativeState::new();
        {
            let mut info = state.cursor_info.lock().expect("cursor info poisoned");
            info.lines = (0..(MAX_INFO_LINES + 20))
                .map(|index| format!("line-{index}"))
                .collect();
        }
        let rendered: Value = serde_json::from_str(&state.info_lines(80)).unwrap();
        assert_eq!(rendered.as_array().map(Vec::len), Some(MAX_INFO_LINES));
    }

    #[test]
    fn no_goal_keeps_cursor_info_callbacks_current() {
        let state = NativeState::new();
        state.activate();
        let generation = state.begin_request("/tmp/Main.lean".into(), 0, 0);
        state.record_callback(generation, "null".into());
        state.record_hover(
            generation,
            r#"{"contents":"Nat","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}}}"#.into(),
        );
        let rendered = serde_json::from_str::<Value>(&state.info_lines(36)).unwrap();
        let text = rendered
            .as_array()
            .into_iter()
            .flat_map(|rows| rows.iter())
            .flat_map(|row| row.as_array().into_iter().flatten())
            .filter_map(|span| span.get("text").and_then(Value::as_str))
            .collect::<String>();
        assert!(text.contains("Nat"));
    }

    #[test]
    fn document_change_invalidates_callback_and_advances_version() {
        let state = NativeState::new();
        state.activate();
        let generation = state.begin_request("/tmp/Main.lean".into(), 1, 0);
        assert_ne!(generation, 0);
        state.record_document_change();
        state.record_callback(
            generation,
            r#"{"goals":["stale"],"rendered":"stale"}"#.into(),
        );
        assert_eq!(
            *state.goal.lock().expect("goal state poisoned"),
            "Lean goal unavailable"
        );
        let reopened = state.begin_request("/tmp/Main.lean".into(), 1, 0);
        assert!(reopened > generation);
    }

    #[test]
    fn rpc_goal_request_preserves_numeric_and_string_session_ids() {
        for session in [r#"1241.0"#, r#""session-7""#] {
            let state = NativeState::new();
            state.activate();
            let generation = state.begin_request("/tmp/Main.lean".into(), 1, 0);
            let result = format!(r#"{{"sessionId":{session}}}"#);
            let request = state.rpc_goals_request(generation, result);
            assert!(request.contains(&format!("\"sessionId\":{session}")));
            assert!(request.contains("Lean.Widget.getInteractiveGoals"));
        }
    }

    #[test]
    fn rpc_session_lifecycle_collects_refs_and_rejects_errors() {
        let state = NativeState::new();
        state.activate();
        let generation = state.begin_request("/tmp/Main.lean".into(), 1, 0);

        state.record_rpc(generation, r#"{"sessionId":17}"#.into());
        assert!(state.rpc_session_current(generation));
        state.record_rpc(generation, r#"{"goals":[],"nested":{"p":1}}"#.into());

        let release: Value =
            serde_json::from_str(&state.rpc_release_request()).expect("release request JSON");
        assert_eq!(release.get("sessionId"), Some(&serde_json::json!(17)));
        assert_eq!(release["refs"].as_array().map(Vec::len), Some(1));
        assert!(!state.rpc_session_current(generation));

        state.record_rpc(generation, r#"{"sessionId":"new"}"#.into());
        assert!(state.rpc_session_current(generation));
        state.record_rpc(generation, r#"{"error":{"code":-32600}}"#.into());
        assert!(!state.rpc_session_current(generation));
    }

    #[test]
    fn rpc_reset_rejects_session_owned_by_restarted_server() {
        let state = NativeState::new();
        state.activate();
        let generation = state.begin_request("/tmp/Main.lean".into(), 1, 0);
        state.record_rpc(generation, r#"{"sessionId":"old"}"#.into());
        assert!(state.rpc_session_current(generation));

        state.reset_rpc_after_server_restart();

        assert!(!state.rpc_session_current(generation));
        assert_eq!(
            state.rpc_goals_request(generation, r#"{"sessionId":"old"}"#.into()),
            "{}"
        );
        let reset_generation = state.generation_number();
        state.reset_rpc_after_server_restart();
        assert_eq!(state.generation_number(), reset_generation);
    }

    #[test]
    fn term_goal_action_is_capability_checked_and_rendered() {
        let state = NativeState::new();
        state.activate();
        let generation = state.begin_request("/tmp/Main.lean".into(), 1, 0);
        state.record_rpc(generation, r#"{"sessionId":"session"}"#.into());
        let request = state.rpc_term_goal_request(generation);
        assert!(request.contains("Lean.Widget.getInteractiveTermGoal"));
        assert!(request.contains("\"sessionId\":\"session\""));
        assert_eq!(state.rpc_term_goal_request(generation), "{}");
        state.record_rpc_action(
            generation,
            r#"{"result":{"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":1}},"term":{"p":"1"},"type":{"tag":[{"info":{"p":"2"},"subexprPos":"/"},{"text":"Nat"}]},"hyps":[],"ctx":{"p":"3"}}}"#.into(),
        );
        let rendered = state.info_lines(36);
        let rendered = serde_json::from_str::<Value>(&rendered).unwrap();
        let text = rendered
            .as_array()
            .into_iter()
            .flat_map(|rows| rows.iter())
            .flat_map(|row| row.as_array().into_iter().flatten())
            .filter_map(|span| span.get("text").and_then(Value::as_str))
            .collect::<String>();
        assert!(text.contains("Nat"));
    }

    #[test]
    fn term_goal_action_rejects_after_restart() {
        let state = NativeState::new();
        state.activate();
        let generation = state.begin_request("/tmp/Main.lean".into(), 1, 0);
        state.record_rpc(generation, r#"{"sessionId":"old"}"#.into());
        state.reset_rpc_after_server_restart();
        assert_eq!(state.rpc_term_goal_request(generation), "{}");
        state.record_rpc_action(
            generation,
            r#"{"result":{"type":{"text":"stale"}}}"#.into(),
        );
        assert_eq!(state.info_lines(36), "[]");
    }

    #[test]
    fn rpc_session_isolated_when_document_changes() {
        let state = NativeState::new();
        state.activate();
        let first = state.begin_request("/tmp/RootA/Main.lean".into(), 1, 0);
        state.record_rpc(first, r#"{"sessionId":"root-a"}"#.into());
        assert!(state.rpc_session_current(first));

        state.record_opened();
        let second = state.begin_request("/tmp/RootB/Main.lean".into(), 1, 0);
        assert_ne!(first, second);
        assert!(!state.rpc_session_current(first));
        assert_eq!(state.rpc_goals_request(first, r#"{"sessionId":"root-a"}"#.into()), "{}");
    }

    #[test]
    fn rpc_sessions_are_isolated_between_live_roots() {
        let first = NativeState::new();
        let second = NativeState::new();
        first.activate();
        second.activate();
        let first_generation = first.begin_request("/tmp/RootA/Main.lean".into(), 1, 0);
        let second_generation = second.begin_request("/tmp/RootB/Main.lean".into(), 2, 0);
        first.record_rpc(first_generation, r#"{"sessionId":"root-a"}"#.into());
        second.record_rpc(second_generation, r#"{"sessionId":"root-b"}"#.into());

        let first_request = first.rpc_term_goal_request(first_generation);
        let second_request = second.rpc_term_goal_request(second_generation);
        assert!(first_request.contains("root-a"));
        assert!(first_request.contains("RootA/Main.lean"));
        assert!(second_request.contains("root-b"));
        assert!(second_request.contains("RootB/Main.lean"));

        first.record_opened();
        assert!(!first.rpc_session_current(first_generation));
        assert!(second.rpc_session_current(second_generation));
    }

    #[test]
    fn cursor_info_renders_hover_and_inlay_in_the_information_surface() {
        let state = NativeState::new();
        state.activate();
        let generation = state.begin_request("/tmp/Main.lean".into(), 0, 3);
        state.record_hover(
            generation,
            r#"{"contents":{"kind":"markdown","value":"Nat"},"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":4}}}"#.into(),
        );
        state.record_inlay(
            generation,
            r#"[{"position":{"line":0,"character":3},"label":" : Nat"}]"#.into(),
        );
        let rendered = state.info_lines(36);
        let rendered = serde_json::from_str::<Value>(&rendered).unwrap();
        let mut text = String::new();
        for row in rendered.as_array().unwrap() {
            for span in row.as_array().unwrap() {
                if let Some(value) = span.get("text").and_then(Value::as_str) {
                    text.push_str(value);
                }
            }
        }
        assert!(text.contains("Nat"));
        assert!(text.contains(": Nat"));
    }

    #[test]
    fn cursor_info_rejects_stale_and_out_of_range_hover() {
        let state = NativeState::new();
        state.activate();
        let generation = state.begin_request("/tmp/Main.lean".into(), 1, 2);
        state.record_hover(
            generation,
            r#"{"contents":"wrong","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}}"#.into(),
        );
        assert_eq!(state.info_lines(36), "[]");
        state.cancel_request();
        state.record_hover(generation, r#"{"contents":"stale"}"#.into());
        assert_eq!(state.info_lines(36), "[]");
    }
}
