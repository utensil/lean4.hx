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
    correspondence::{first_location, same_document},
    goals::GoalState,
    protocol::{Extras, PlainGoal, Position},
};

pub mod actions;
pub mod correspondence;
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
    focused: AtomicBool,
    scroll: AtomicUsize,
    last_request: Mutex<String>,
    last_callback: Mutex<String>,
    goal: Mutex<String>,
    navigation: Mutex<String>,
    current_uri: Mutex<String>,
    goal_state: Mutex<GoalState>,
}

impl Custom for NativeState {}

impl NativeState {
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
            focused: AtomicBool::new(false),
            scroll: AtomicUsize::new(0),
            last_request: Mutex::new("none".to_owned()),
            last_callback: Mutex::new("none".to_owned()),
            goal: Mutex::new("Lean goal unavailable".to_owned()),
            navigation: Mutex::new("Navigation unavailable".to_owned()),
            current_uri: Mutex::new(String::new()),
            goal_state: Mutex::new(GoalState::default()),
        }
    }

    fn activate(&self) {
        self.active.store(true, Ordering::Release);
        self.installs.fetch_add(1, Ordering::Relaxed);
        eprintln!("LEAN4_HX_INSTALL active=true");
    }

    fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.request_inflight.store(false, Ordering::Release);
        self.focused.store(false, Ordering::Release);
        self.scroll.store(0, Ordering::Release);
        self.goal_state.lock().expect("goal state poisoned").close();
        *self.goal.lock().expect("goal state poisoned") = "Lean goal unavailable".to_owned();
        *self.navigation.lock().expect("navigation state poisoned") =
            "Navigation unavailable".to_owned();
        self.removals.fetch_add(1, Ordering::Relaxed);
        eprintln!("LEAN4_HX_REMOVE active=false");
    }

    fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    fn record_selection(&self) {
        if self.is_active() {
            self.selections.fetch_add(1, Ordering::Relaxed);
            eprintln!("LEAN4_HX_SELECTION");
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
        self.scroll.store(0, Ordering::Release);
        self.goal_state
            .lock()
            .expect("goal state poisoned")
            .invalidate_and_advance("document changed");
        *self.goal.lock().expect("goal state poisoned") = "Lean goal unavailable".to_owned();
        *self.navigation.lock().expect("navigation state poisoned") =
            "Navigation unavailable".to_owned();
        self.inserts.fetch_add(1, Ordering::Relaxed);
        eprintln!(
            "LEAN4_HX_DOCUMENT_CHANGED version={}",
            self.document_version.load(Ordering::Acquire)
        );
    }

    fn record_opened(&self) {
        if self.is_active() {
            self.generation.fetch_add(1, Ordering::AcqRel);
            self.document_version.store(1, Ordering::Release);
            self.request_inflight.store(false, Ordering::Release);
            self.scroll.store(0, Ordering::Release);
            self.goal_state.lock().expect("goal state poisoned").close();
            *self.goal.lock().expect("goal state poisoned") = "Lean goal unavailable".to_owned();
            *self.navigation.lock().expect("navigation state poisoned") =
                "Navigation unavailable".to_owned();
            self.opened.fetch_add(1, Ordering::Relaxed);
            eprintln!("LEAN4_HX_OPEN");
        }
    }

    fn record_closed(&self) {
        if self.is_active() {
            self.generation.fetch_add(1, Ordering::AcqRel);
            self.request_inflight.store(false, Ordering::Release);
            self.focused.store(false, Ordering::Release);
            self.scroll.store(0, Ordering::Release);
            self.goal_state.lock().expect("goal state poisoned").close();
            *self.goal.lock().expect("goal state poisoned") = "Lean goal unavailable".to_owned();
            *self.navigation.lock().expect("navigation state poisoned") =
                "Navigation unavailable".to_owned();
            self.closed.fetch_add(1, Ordering::Relaxed);
            eprintln!("LEAN4_HX_CLOSE");
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
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.request_inflight.store(false, Ordering::Release);
        self.scroll.store(0, Ordering::Release);
        self.goal_state
            .lock()
            .expect("goal state poisoned")
            .invalidate_and_advance("selection changed");
        *self.navigation.lock().expect("navigation state poisoned") =
            "Navigation unavailable".to_owned();
    }

    fn record_request(&self, path: String, line: usize, character: usize) {
        if self.is_active() {
            self.requests.fetch_add(1, Ordering::Relaxed);
            let mut last = self.last_request.lock().expect("request state poisoned");
            *last = format!("line={line},character={character},uri={}", file_uri(&path));
            eprintln!(
                "LEAN4_HX_REQUEST line={line} character={character} uri={}",
                file_uri(&path)
            );
        }
    }

    fn record_callback(&self, generation: usize, result: String) {
        if self.is_active() && self.generation.load(Ordering::Acquire) == generation {
            self.callbacks.fetch_add(1, Ordering::Relaxed);
            let mut last = self.last_callback.lock().expect("callback state poisoned");
            *last = "reply".to_owned();
            let goal = if result == "null" {
                self.goal_state
                    .lock()
                    .expect("goal state poisoned")
                    .invalidate("Lean returned no goal");
                "Lean goal unavailable".to_owned()
            } else {
                let accepted = serde_json::from_str::<Value>(&result)
                    .ok()
                    .and_then(|value| Self::plain_goal(&value))
                    .map(|plain| {
                        self.goal_state
                            .lock()
                            .expect("goal state poisoned")
                            .accept_plain_goal(generation as u64, plain)
                    })
                    .unwrap_or(false);
                if !accepted {
                    eprintln!("LEAN4_HX_CALLBACK result=stale");
                    return;
                }
                self.goal_state
                    .lock()
                    .expect("goal state poisoned")
                    .display_text()
            };
            self.request_inflight.store(false, Ordering::Release);
            self.scroll.store(0, Ordering::Release);
            *self.goal.lock().expect("goal state poisoned") = goal.clone();
            eprintln!("LEAN4_HX_CALLBACK result=reply");
            eprintln!("LEAN4_HX_GOAL generation={generation} text={goal}");
            if goal != "Lean goal unavailable" && !goal.is_empty() {
                let lines = self
                    .goal_state
                    .lock()
                    .expect("goal state poisoned")
                    .snapshot()
                    .map(|snapshot| snapshot.display_lines().len())
                    .unwrap_or(0);
                eprintln!("LEAN4_HX_GOAL_RENDER_READY lines={lines} available=true");
                eprintln!("LEAN4_HX_GOAL_AVAILABLE");
            }
        } else {
            eprintln!("LEAN4_HX_CALLBACK result=stale");
        }
    }

    fn record_navigation(&self, generation: usize, result: String) {
        if !self.is_active() || self.generation.load(Ordering::Acquire) != generation {
            eprintln!("LEAN4_HX_NAVIGATION stale");
            return;
        }
        let current_uri = self
            .current_uri
            .lock()
            .expect("navigation state poisoned")
            .clone();
        let navigation = serde_json::from_str::<Value>(&result)
            .ok()
            .and_then(|value| first_location(&value))
            .map(|location| {
                let relation = if same_document(&current_uri, &location) {
                    "same-file"
                } else {
                    "cross-file"
                };
                eprintln!("LEAN4_HX_NAVIGATION {relation} uri={}", location.uri);
                format!("navigation {relation}")
            })
            .unwrap_or_else(|| {
                eprintln!("LEAN4_HX_NAVIGATION unavailable");
                "navigation unavailable".to_owned()
            });
        *self.navigation.lock().expect("navigation state poisoned") = navigation;
    }

    fn record_rpc(&self, generation: usize, result: String) {
        if self.is_active() && self.generation.load(Ordering::Acquire) == generation {
            if result.contains("sessionId") {
                eprintln!("LEAN4_HX_RPC action=connect");
            } else {
                eprintln!("LEAN4_HX_RPC action=reply");
            }
        } else {
            eprintln!("LEAN4_HX_RPC stale");
        }
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
        eprintln!("LEAN4_HX_RENDER {goal}");
        format!("lean4.hx goal {renders}: {goal} [{navigation}]")
    }

    fn lines(&self) -> Vec<String> {
        let mut lines = vec![if self.focused() {
            "Lean goals [focused]".to_owned()
        } else {
            "Lean goals [source]".to_owned()
        }];
        let body = self
            .goal_state
            .lock()
            .expect("goal state poisoned")
            .snapshot()
            .map(|snapshot| snapshot.display_lines())
            .unwrap_or_else(|| vec!["Lean goal unavailable".to_owned()]);
        let offset = self.scroll.load(Ordering::Acquire);
        lines.extend(body.into_iter().skip(offset));
        lines
    }

    fn focused(&self) -> bool {
        self.focused.load(Ordering::Acquire)
    }

    fn set_focused(&self, focused: bool) {
        self.focused.store(focused, Ordering::Release);
        eprintln!("LEAN4_HX_GOAL_FOCUS focused={focused}");
    }

    fn scroll(&self, amount: isize) {
        let current = self.scroll.load(Ordering::Acquire) as isize;
        self.scroll.store(
            current.saturating_add(amount).max(0) as usize,
            Ordering::Release,
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
        .register_fn("native-record-request!", NativeState::record_request)
        .register_fn("native-begin-request!", NativeState::begin_request)
        .register_fn("native-cancel-request!", NativeState::cancel_request)
        .register_fn("native-record-callback!", NativeState::record_callback)
        .register_fn("native-record-navigation!", NativeState::record_navigation)
        .register_fn("native-record-rpc!", NativeState::record_rpc)
        .register_fn("native-label", NativeState::label)
        .register_fn("native-lines", NativeState::lines)
        .register_fn("native-set-focused!", NativeState::set_focused)
        .register_fn("native-scroll!", NativeState::scroll)
        .register_fn("native-summary", NativeState::summary)
        .register_fn("native-unicode-start", NativeState::unicode_start)
        .register_fn("native-unicode-end", NativeState::unicode_end)
        .register_fn(
            "native-unicode-replacement",
            NativeState::unicode_replacement,
        )
        .register_fn("native-unicode-cursor", NativeState::unicode_cursor)
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
        let generation = state.begin_request("/tmp/Main.lean".into(), 1, 0);
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
}
